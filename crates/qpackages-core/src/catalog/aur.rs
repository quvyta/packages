//! Asking the AUR what it has: request URLs for its RPC, and reading the answers.
//!
//! The RPC (version 5) answers `search` with the summary of every match and `info` with the full
//! record, dependencies included, for the names asked for. The store searches first and then asks
//! `info` about the first results in a few batched requests. Answers are read field by field, so a
//! field the AUR adds or drops changes nothing here.

use std::fmt;

use tinyjson::JsonValue;

use super::Problem;
use super::json;
use super::net::percent_encode;

/// Where the RPC lives.
const RPC: &str = "https://aur.archlinux.org/rpc/v5";

/// The fewest characters the RPC accepts in a search term; shorter ones are refused by the
/// server as too broad.
pub const MIN_SEARCH_CHARS: usize = 2;

/// The longest `info` URL built. The server refused a request of 9,000 bytes and took one of
/// 8,000; this stays well under both so a proxy with a smaller limit does not break it.
pub const MAX_URL_LEN: usize = 4400;

/// What a search term is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBy {
    /// Package names only.
    Name,
    /// Package names and descriptions; what the store's search box uses.
    NameDesc,
}

impl SearchBy {
    /// The value of the RPC's `by` parameter.
    const fn parameter(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::NameDesc => "name-desc",
        }
    }
}

/// The URL of a search for `term`, or `None` when the term is too short for the RPC to accept.
/// Surrounding whitespace does not count toward the length and is not sent.
#[must_use]
pub fn search_url(term: &str, by: SearchBy) -> Option<String> {
    let term = term.trim();
    (term.chars().count() >= MIN_SEARCH_CHARS)
        .then(|| format!("{RPC}/search/{}?by={}", percent_encode(term), by.parameter()))
}

/// The URLs of `info` requests covering every name in `names`, in order, each no longer than
/// [`MAX_URL_LEN`]. A name too long to fit a request on its own cannot be an AUR package name
/// (those are at most a few dozen characters) and is left out.
#[must_use]
pub fn info_urls<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let base = format!("{RPC}/info?");
    let mut urls = Vec::new();
    let mut current = String::new();
    for name in names {
        let argument = format!("arg%5B%5D={}", percent_encode(name.as_ref()));
        if base.len() + argument.len() > MAX_URL_LEN {
            continue;
        }
        if !current.is_empty() && current.len() + 1 + argument.len() > MAX_URL_LEN {
            urls.push(std::mem::take(&mut current));
        }
        if current.is_empty() {
            current.push_str(&base);
        } else {
            current.push('&');
        }
        current.push_str(&argument);
    }
    if !current.is_empty() {
        urls.push(current);
    }
    urls
}

/// One package as the RPC describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AurPackage {
    /// The package name.
    pub name: String,
    /// The package base it is built from; several packages can share one.
    pub base: String,
    /// The version, including the release.
    pub version: String,
    /// The one-line description.
    pub description: Option<String>,
    /// How many people voted for it.
    pub votes: u64,
    /// The RPC's popularity: votes weighted by how recent they are.
    pub popularity: f64,
    /// The maintainer's AUR user name; `None` for an orphaned package.
    pub maintainer: Option<String>,
    /// When it was flagged out of date, as a Unix timestamp; `None` when it is not.
    pub out_of_date: Option<i64>,
    /// When it last changed, as a Unix timestamp.
    pub last_modified: Option<i64>,
    /// The upstream project's home page.
    pub url: Option<String>,
    /// What it needs to run. Only `info` answers carry this; after a `search` it is empty.
    pub depends: Vec<String>,
    /// What it needs to build. Only `info` answers carry this.
    pub make_depends: Vec<String>,
    /// What it needs to run its own tests while it is built. Only `info` answers carry this.
    pub check_depends: Vec<String>,
    /// What it can use, each entry as the package writes it. Only `info` answers carry this.
    pub opt_depends: Vec<String>,
    /// Its licences, as the recipe names them. Only `info` answers carry this.
    pub licenses: Vec<String>,
}

/// Why an RPC answer could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AurError {
    /// The answer is not the JSON the RPC sends.
    Malformed(Problem),
    /// The RPC answered with an error of its own, such as `Too many package results.` for a term
    /// that matches thousands of packages. The message is the server's, in English.
    Rpc(String),
}

impl fmt::Display for AurError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(problem) => write!(formatter, "the AUR's answer could not be read: {problem}"),
            Self::Rpc(message) => write!(formatter, "the AUR refused the request: {message}"),
        }
    }
}

impl std::error::Error for AurError {}

/// Reads a `search` or `info` answer.
///
/// A result without a name or a version is skipped; every other field is optional and takes its
/// empty value when missing or of the wrong type. An empty result list is an answer, not an error.
pub fn parse_response(json: &str) -> Result<Vec<AurPackage>, AurError> {
    let value = json::parse(json).map_err(AurError::Malformed)?;
    if json::text(&value, "type") == Some("error") {
        let message = json::text(&value, "error").unwrap_or("unknown error");
        return Err(AurError::Rpc(message.to_owned()));
    }
    let Some(results) = json::array(&value, "results") else {
        return Err(AurError::Malformed(Problem { line: 1, column: 1, message: String::from("no `results` list") }));
    };
    Ok(results.iter().filter_map(package).collect())
}

/// One entry of `results`, or `None` when it lacks what makes it a package.
fn package(entry: &JsonValue) -> Option<AurPackage> {
    let text = |key: &str| json::text(entry, key).map(str::to_owned);
    let name = text("Name")?;
    let version = text("Version")?;
    Some(AurPackage {
        base: text("PackageBase").unwrap_or_else(|| name.clone()),
        name,
        version,
        description: text("Description"),
        votes: json::count(entry, "NumVotes").unwrap_or(0),
        popularity: json::number(entry, "Popularity").unwrap_or(0.0),
        maintainer: text("Maintainer"),
        out_of_date: json::integer(entry, "OutOfDate"),
        last_modified: json::integer(entry, "LastModified"),
        url: text("URL"),
        depends: json::strings(entry, "Depends"),
        make_depends: json::strings(entry, "MakeDepends"),
        check_depends: json::strings(entry, "CheckDepends"),
        opt_depends: json::strings(entry, "OptDepends"),
        licenses: json::strings(entry, "License"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../../tests/fixtures/catalog/aur-search-obs.json");
    const INFO: &str = include_str!("../../tests/fixtures/catalog/aur-info.json");
    const EMPTY: &str = include_str!("../../tests/fixtures/catalog/aur-search-empty.json");
    const TOO_MANY: &str = include_str!("../../tests/fixtures/catalog/aur-error-too-many.json");

    #[test]
    fn a_search_url_encodes_its_term() {
        assert_eq!(
            search_url("obs", SearchBy::NameDesc).as_deref(),
            Some("https://aur.archlinux.org/rpc/v5/search/obs?by=name-desc")
        );
        assert_eq!(
            search_url("  c++ ide/x ", SearchBy::Name).as_deref(),
            Some("https://aur.archlinux.org/rpc/v5/search/c%2B%2B%20ide%2Fx?by=name")
        );
        assert_eq!(
            search_url("çö", SearchBy::Name).as_deref(),
            Some("https://aur.archlinux.org/rpc/v5/search/%C3%A7%C3%B6?by=name")
        );
    }

    #[test]
    fn a_term_shorter_than_two_characters_is_not_sent() {
        assert_eq!(search_url("o", SearchBy::NameDesc), None);
        assert_eq!(search_url(" o ", SearchBy::NameDesc), None);
        assert_eq!(search_url("", SearchBy::NameDesc), None);
        assert!(search_url("ç", SearchBy::NameDesc).is_none(), "one character, however many bytes");
    }

    #[test]
    fn info_asks_for_every_name_in_one_request_when_they_fit() {
        let urls = info_urls(&["obs-vkcapture", "visual-studio-code-bin", "obs-studio-tytan652"]);
        assert_eq!(
            urls,
            [
                "https://aur.archlinux.org/rpc/v5/info?arg%5B%5D=obs-vkcapture&arg%5B%5D=visual-studio-code-bin&arg%5B%5D=obs-studio-tytan652"
            ]
        );
        assert!(info_urls::<&str>(&[]).is_empty());
    }

    #[test]
    fn info_splits_into_requests_that_stay_under_the_limit() {
        let names: Vec<String> = (0..600).map(|index| format!("some-package-name-{index:04}")).collect();
        let urls = info_urls(&names);
        assert!(urls.len() > 1);
        assert!(urls.iter().all(|url| url.len() <= MAX_URL_LEN), "every request fits");
        let asked: Vec<&str> = urls
            .iter()
            .flat_map(|url| url.split_once('?').expect("a query").1.split('&'))
            .map(|argument| argument.strip_prefix("arg%5B%5D=").expect("every argument is a name"))
            .collect();
        assert_eq!(asked, names, "every name once, in order");
    }

    #[test]
    fn a_name_that_cannot_fit_is_left_out() {
        let huge = "x".repeat(MAX_URL_LEN);
        let urls = info_urls(&["yay", huge.as_str(), "paru"]);
        assert_eq!(urls, ["https://aur.archlinux.org/rpc/v5/info?arg%5B%5D=yay&arg%5B%5D=paru"]);
    }

    #[test]
    fn reads_a_recorded_search() {
        let packages = parse_response(SEARCH).expect("a real answer");
        assert_eq!(packages.len(), 20);
        let git = packages.iter().find(|package| package.name == "obs-studio-git").expect("obs-studio-git");
        assert_eq!(git.version, "32.2.2.r4.g1bf1379-1");
        assert_eq!(git.votes, 112);
        assert!((git.popularity - 0.030_016).abs() < 1e-9);
        assert_eq!(git.maintainer.as_deref(), Some("benklett"));
        assert_eq!(git.out_of_date, None);
        assert_eq!(git.last_modified, Some(1_789_517_803));
        assert!(git.depends.is_empty(), "search answers carry no dependencies");

        let boris = packages
            .iter()
            .find(|package| package.name == "behavioral-observation-research-interactive-software")
            .expect("an orphaned, out-of-date package");
        assert_eq!(boris.maintainer, None);
        assert_eq!(boris.out_of_date, Some(1_675_077_754));
        assert_eq!(boris.popularity, 0.0, "an integer 0 reads as a number too");
    }

    #[test]
    fn reads_a_recorded_info_with_dependencies() {
        let packages = parse_response(INFO).expect("a real answer");
        let names: Vec<&str> = packages.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(names, ["obs-vkcapture", "visual-studio-code-bin", "obs-studio-tytan652"]);
        let capture = &packages[0];
        assert_eq!(capture.depends, ["vulkan-icd-loader", "libgl", "libegl", "obs-studio>=28"]);
        assert_eq!(capture.make_depends.len(), 7);
        assert!(capture.opt_depends.is_empty(), "a null list is an empty one");
        assert_eq!(capture.url.as_deref(), Some("https://github.com/nowrep/obs-vkcapture"));
        let code = &packages[1];
        assert_eq!(code.votes, 1708);
        assert_eq!(code.base, "visual-studio-code-bin");
        assert_eq!(code.opt_depends, ["glib2", "libdbusmenu-glib", "org.freedesktop.secrets", "icu69"]);
        assert_eq!(capture.licenses, ["GPL-2.0-or-later"]);
        assert_eq!(code.licenses, ["custom: commercial"]);
    }

    #[test]
    fn the_dependencies_a_build_runs_its_tests_with_are_read() {
        let json = r#"{"type":"multiinfo","results":[{"Name":"a","Version":"1-1","CheckDepends":["python-pytest"]}]}"#;
        let packages = parse_response(json).expect("readable");
        assert_eq!(packages[0].check_depends, ["python-pytest"]);
        assert!(parse_response(INFO).expect("a real answer")[0].check_depends.is_empty(), "absent is empty");
    }

    #[test]
    fn no_results_is_an_empty_answer() {
        assert_eq!(parse_response(EMPTY), Ok(Vec::new()));
    }

    #[test]
    fn the_rpcs_own_error_is_passed_on() {
        assert_eq!(parse_response(TOO_MANY), Err(AurError::Rpc(String::from("Too many package results."))));
    }

    #[test]
    fn a_broken_answer_is_an_error_with_a_position() {
        let Err(AurError::Malformed(problem)) = parse_response("{\"results\": [\n  {\"Name\": }\n]}") else {
            panic!("broken JSON must be reported");
        };
        assert_eq!(problem.line, 2);
        assert!(matches!(parse_response("<html>502</html>"), Err(AurError::Malformed(_))));
        assert!(matches!(parse_response("{}"), Err(AurError::Malformed(_))));
    }

    #[test]
    fn a_result_without_a_name_or_version_is_skipped() {
        let json = r#"{"type":"search","results":[{"Name":"a"},{"Version":"1"},{"Name":"b","Version":"2","NumVotes":"many"}]}"#;
        let packages = parse_response(json).expect("readable");
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "b");
        assert_eq!(packages[0].votes, 0, "a field of the wrong type takes its empty value");
        assert_eq!(packages[0].base, "b", "without a base the package is its own");
    }

    #[test]
    fn reads_what_the_tests_need_to_build() {
        let json = r#"{"type":"multiinfo","results":[{"Name":"a","Version":"1","CheckDepends":["python-pytest"]}]}"#;
        let packages = parse_response(json).expect("readable");
        assert_eq!(packages[0].check_depends, ["python-pytest"]);
    }
}

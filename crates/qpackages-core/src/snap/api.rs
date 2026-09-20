//! snapd's own interface: the requests qpackages writes to [`super::SOCKET`] and the JSON that
//! comes back.
//!
//! Everything here reads as the ordinary user. snapd lets a user list, search and watch a job's
//! progress without any permission — even a job root started — so the whole reading side of Snap
//! needs no helper and no password.
//!
//! Nothing here opens a socket: the caller writes what [`request`] returns and hands back what it
//! read. The request is HTTP/1.0 with `Connection: close`, because snapd then answers in HTTP/1.0
//! too and sends the body in one piece, ending it by closing the connection. An HTTP/1.1 request
//! gets `Transfer-Encoding: chunked` on the larger answers, and unpicking that framing would be
//! code with nothing to gain. What is left is a request of four lines and a body that starts after
//! the first blank line.
//!
//! The `snap` program's own output is not read anywhere: it has no JSON option, and its tables are
//! aligned with spaces that also appear inside a summary, so no column can be found again.
//! Measured against snapd 2.77.1; the answers are recorded under `tests/fixtures/snap`.

use tinyjson::JsonValue;

use crate::catalog::Problem;
use crate::catalog::json;

/// What snapd says about itself, including its version.
pub const SYSTEM_INFO: &str = "/v2/system-info";

/// The installed snaps.
pub const SNAPS: &str = "/v2/snaps";

/// The store's categories, as a list of objects with a `name`.
pub const CATEGORIES: &str = "/v2/categories";

/// The path that searches the store for `term`.
///
/// `scope=wide` is what the `snap find` command sends: without it snapd narrows the answer to
/// what it thinks fits this machine, and a search then misses snaps the user could still install.
#[must_use]
pub fn find_path(term: &str) -> String {
    format!("/v2/find?q={}&scope=wide", escaped(term))
}

/// The path that asks the store about one snap by name, which is how an application page is
/// filled: one record with its channels and its confinement.
#[must_use]
pub fn name_path(name: &str) -> String {
    format!("/v2/find?name={}", escaped(name))
}

/// The path of job `id`, whose tasks carry how far it has come.
#[must_use]
pub fn change_path(id: u32) -> String {
    format!("/v2/changes/{id}")
}

/// The request to write to the socket for `path`, ended by its own blank line.
#[must_use]
pub fn request(path: &str) -> String {
    format!("GET {path} HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\nConnection: close\r\n\r\n")
}

/// `text` with everything but the characters a URL may carry unescaped written as `%XX`.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The body of what the socket answered: everything after the first blank line.
///
/// # Errors
///
/// Returns why the answer is not one to read: no status line, a status other than 200, or no
/// blank line ending the headers.
pub fn body(answer: &str) -> Result<&str, Problem> {
    let Some(status) = answer.lines().next() else {
        return Err(Problem::at(answer, 0, "snapd answered nothing"));
    };
    // `HTTP/1.0 200 OK`: the middle word is the code.
    let code = status.split(' ').nth(1).unwrap_or_default();
    if code != "200" {
        return Err(Problem::at(answer, 0, format!("snapd answered `{}`", status.trim())));
    }
    let end = ["\r\n\r\n", "\n\n"].iter().filter_map(|mark| answer.find(mark).map(|at| at + mark.len())).min();
    let Some(end) = end else {
        return Err(Problem::at(answer, answer.len(), "the headers never end"));
    };
    Ok(&answer[end..])
}

/// The `result` of a JSON answer, whatever shape it has.
///
/// # Errors
///
/// Returns why the text is not an answer with a result in it.
fn result(text: &str) -> Result<JsonValue, Problem> {
    let value = json::parse(text)?;
    json::field(&value, "result").cloned().ok_or_else(|| Problem::at(text, 0, "the answer carries no result"))
}

/// What snapd is, as much of it as the screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    /// snapd's version, as the settings row shows it.
    pub version: String,
    /// The store series snapd speaks, `16` at the time of writing.
    pub series: String,
}

/// Reads [`SYSTEM_INFO`]'s answer.
///
/// # Errors
///
/// Returns why the answer could not be read.
pub fn parse_system_info(text: &str) -> Result<SystemInfo, Problem> {
    let result = result(text)?;
    let version = json::text(&result, "version")
        .ok_or_else(|| Problem::at(text, 0, "the system info carries no version"))?
        .to_owned();
    Ok(SystemInfo { version, series: json::text(&result, "series").unwrap_or_default().to_owned() })
}

/// What a snap is: an application, or a piece of the machinery snaps run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// An application, the only kind the store shows a card for.
    App,
    /// A base snap: the filesystem an application runs on.
    Base,
    /// The `core` snap, the older base that is also an operating system image.
    Os,
    /// snapd itself, installed as a snap.
    Snapd,
    /// A kind snapd named that qpackages does not act on.
    Other(String),
}

impl Kind {
    fn from_text(text: &str) -> Self {
        match text {
            "app" => Self::App,
            "base" => Self::Base,
            "os" => Self::Os,
            "snapd" => Self::Snapd,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Whether this is something the user installed for its own sake, rather than a base snap or
    /// snapd that came along with it. Only these become cards; the rest is shown as what a snap
    /// runs on.
    #[must_use]
    pub const fn is_app(&self) -> bool {
        matches!(self, Self::App)
    }
}

/// How closely a snap is held to its sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// Inside the sandbox, which is the usual case.
    Strict,
    /// Outside it: the snap can change anything on the machine. Installing one needs the user to
    /// say so, and the confirmation says what it means.
    Classic,
    /// A developer's snap, sandboxed but allowed to break out for debugging.
    Devmode,
}

impl Confinement {
    fn from_text(text: &str) -> Option<Self> {
        match text {
            "strict" => Some(Self::Strict),
            "classic" => Some(Self::Classic),
            "devmode" => Some(Self::Devmode),
            _ => None,
        }
    }
}

/// One snap, installed or in the store: both answers carry the same record, only some fields
/// filled differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snap {
    /// The name that installs and removes it.
    pub name: String,
    /// The name shown to people; the name itself when the store has no other.
    pub title: String,
    /// One line about it, when there is one.
    pub summary: Option<String>,
    /// Its version, as its publisher wrote it.
    pub version: String,
    /// What it is.
    pub kind: Kind,
    /// How closely it is confined; `None` when snapd did not say.
    pub confinement: Option<Confinement>,
    /// Who publishes it, by their shown name.
    pub publisher: Option<String>,
    /// Whether the store verified that publisher.
    pub verified: bool,
    /// How much of the disk it takes once installed, when snapd said.
    pub installed_size: Option<u64>,
    /// How much there is to download, when snapd said.
    pub download_size: Option<u64>,
    /// Where its icon is, from the media the store carries.
    pub icon_url: Option<String>,
    /// When it was installed, as snapd wrote the time; `None` for a snap that is not.
    pub install_date: Option<String>,
}

impl Snap {
    /// Whether the snap runs outside the sandbox, which a confirmation has to say.
    #[must_use]
    pub fn is_classic(&self) -> bool {
        self.confinement == Some(Confinement::Classic)
    }
}

/// Reads one snap's record, as both [`SNAPS`] and [`find_path`] carry it. A record without a name
/// is no record.
fn snap(value: &JsonValue) -> Option<Snap> {
    let name = json::text(value, "name")?.to_owned();
    let publisher = json::field(value, "publisher");
    let shown = |key: &str| publisher.and_then(|it| json::text(it, key)).filter(|text| !text.is_empty());
    let media = json::array(value, "media").map(Vec::as_slice).unwrap_or_default();
    let icon_url = media
        .iter()
        .find(|item| json::text(item, "type") == Some("icon"))
        .and_then(|item| json::text(item, "url"))
        .map(str::to_owned);
    let text = |key: &str| json::text(value, key).filter(|it| !it.is_empty()).map(str::to_owned);
    Some(Snap {
        title: text("title").unwrap_or_else(|| name.clone()),
        name,
        summary: text("summary"),
        version: text("version").unwrap_or_default(),
        kind: Kind::from_text(json::text(value, "type").unwrap_or_default()),
        confinement: json::text(value, "confinement").and_then(Confinement::from_text),
        publisher: shown("display-name").or_else(|| shown("username")).map(str::to_owned),
        verified: shown("validation") == Some("verified"),
        installed_size: json::count(value, "installed-size"),
        download_size: json::count(value, "download-size"),
        icon_url,
        install_date: text("install-date"),
    })
}

/// Reads an answer whose result is a list of snaps: the installed ones, or what a search found.
/// A record that cannot be read is left out and reported; the rest of the list stands.
///
/// # Errors
///
/// Returns why the answer as a whole could not be read.
pub fn parse_snaps(text: &str) -> Result<(Vec<Snap>, Vec<Problem>), Problem> {
    let result = result(text)?;
    let Some(items) = result.get::<Vec<JsonValue>>() else {
        return Err(Problem::at(text, 0, "the result is not a list of snaps"));
    };
    let mut snaps = Vec::new();
    let mut problems = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match snap(item) {
            Some(read) => snaps.push(read),
            None => problems.push(Problem::at(text, 0, format!("snap {index} has no name"))),
        }
    }
    Ok((snaps, problems))
}

/// Reads [`CATEGORIES`]' answer: the store's category names, in the order snapd lists them.
///
/// # Errors
///
/// Returns why the answer could not be read.
pub fn parse_categories(text: &str) -> Result<Vec<String>, Problem> {
    let result = result(text)?;
    let Some(items) = result.get::<Vec<JsonValue>>() else {
        return Err(Problem::at(text, 0, "the result is not a list of categories"));
    };
    Ok(items.iter().filter_map(|item| json::text(item, "name")).map(str::to_owned).collect())
}

/// One step of a job snapd is carrying out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// What sort of step it is, such as `download-snap`.
    pub kind: String,
    /// The line snapd wrote about it, which is what the status line shows.
    pub summary: String,
    /// `Do` before it starts, `Doing` while it runs, `Done` when it is over.
    pub status: String,
    /// What the numbers below count: the snap's name while it downloads, empty otherwise.
    pub label: String,
    /// How much is behind it.
    pub done: u64,
    /// How much there is; `1` for a step that only ends.
    pub total: u64,
}

impl Task {
    /// Whether this step is the one running now.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.status == "Doing"
    }

    /// How far along it is, from 0 to 1, when the numbers mean something.
    ///
    /// A download counts bytes, so the share is real. Every other step counts one of one, which
    /// says nothing about how long it will take: there the bar is left unknown.
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 1).then(|| {
            #[expect(clippy::cast_precision_loss, reason = "a share to draw a bar with, not a byte count")]
            let share = self.done.min(self.total) as f64 / self.total as f64;
            share
        })
    }
}

/// A job snapd is carrying out or has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Its number, the one `--no-wait` printed.
    pub id: String,
    /// What sort of job it is, such as `install-snap`.
    pub kind: String,
    /// The line snapd wrote about the job as a whole.
    pub summary: String,
    /// `Doing` while it runs, `Done` when every step went well.
    pub status: String,
    /// Whether it is over, however it ended.
    pub ready: bool,
    /// Its steps, in the order snapd runs them.
    pub tasks: Vec<Task>,
}

impl Change {
    /// The step running now, whose summary the status line shows.
    #[must_use]
    pub fn running(&self) -> Option<&Task> {
        self.tasks.iter().find(|task| task.is_running())
    }

    /// Whether every step went well.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.status == "Done"
    }

    /// Whether the job is over and did not end well. Only a job snapd calls ready and does not
    /// call `Done` failed; while it runs nothing is claimed.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.ready && !self.succeeded()
    }
}

/// Reads [`change_path`]'s answer.
///
/// # Errors
///
/// Returns why the answer could not be read.
pub fn parse_change(text: &str) -> Result<Change, Problem> {
    let result = result(text)?;
    let Some(id) = json::text(&result, "id") else {
        return Err(Problem::at(text, 0, "the job carries no number"));
    };
    let word = |value: &JsonValue, key: &str| json::text(value, key).unwrap_or_default().to_owned();
    let tasks = json::array(&result, "tasks")
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|task| {
            let progress = json::field(task, "progress");
            let count = |key: &str| progress.and_then(|it| json::count(it, key));
            Task {
                kind: word(task, "kind"),
                summary: word(task, "summary"),
                status: word(task, "status"),
                label: progress.map(|it| word(it, "label")).unwrap_or_default(),
                done: count("done").unwrap_or_default(),
                total: count("total").unwrap_or_default(),
            }
        })
        .collect();
    Ok(Change {
        id: id.to_owned(),
        kind: word(&result, "kind"),
        summary: word(&result, "summary"),
        status: word(&result, "status"),
        ready: json::field(&result, "ready").and_then(|it| it.get::<bool>().copied()).unwrap_or_default(),
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/snap").join(name);
        std::fs::read_to_string(path).expect("the recording is readable")
    }

    #[test]
    fn a_request_is_four_lines_of_http_one_zero() {
        assert_eq!(
            request(SNAPS),
            "GET /v2/snaps HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn a_search_term_is_escaped_and_the_scope_is_the_wide_one() {
        assert_eq!(find_path("hello"), "/v2/find?q=hello&scope=wide");
        assert_eq!(find_path("hello world"), "/v2/find?q=hello%20world&scope=wide");
        assert_eq!(find_path("a&b=c#d"), "/v2/find?q=a%26b%3Dc%23d&scope=wide");
        assert_eq!(find_path("çay"), "/v2/find?q=%C3%A7ay&scope=wide", "every byte of a letter on its own");
        assert_eq!(name_path("hello-world"), "/v2/find?name=hello-world");
        assert_eq!(change_path(10), "/v2/changes/10");
    }

    #[test]
    fn the_body_starts_after_the_blank_line_and_another_status_is_refused() {
        let answer = "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"result\":1}";
        assert_eq!(body(answer).expect("a body"), "{\"result\":1}");
        assert_eq!(body("HTTP/1.0 200 OK\n\n{}").expect("a body"), "{}");
        let refused = body("HTTP/1.0 404 Not Found\r\n\r\n{}").expect_err("not a body");
        assert!(refused.message.contains("404"), "{}", refused.message);
        assert!(body("HTTP/1.0 200 OK\r\nContent-Type: x\r\n").is_err(), "the headers never end");
        assert!(body("").is_err());
    }

    #[test]
    fn snapds_version_is_read_from_the_recorded_system_info() {
        let info = parse_system_info(&fixture("api-system-info.json")).expect("the recording reads");
        assert_eq!(info, SystemInfo { version: String::from("2.77.1-1"), series: String::from("16") });
        assert!(parse_system_info("{\"result\":{}}").is_err(), "no version is no answer");
        assert!(parse_system_info("{}").is_err(), "no result is no answer");
        assert!(parse_system_info("not json").is_err());
    }

    #[test]
    fn the_installed_snaps_are_read_with_the_bases_told_from_the_applications() {
        let (snaps, problems) = parse_snaps(&fixture("api-snaps.json")).expect("the recording reads");
        assert_eq!(problems, []);
        assert_eq!(snaps.len(), 7);
        let hello = snaps.iter().find(|snap| snap.name == "hello-world").expect("it is installed");
        assert_eq!(hello.title, "Hello World");
        assert_eq!(hello.version, "6.4");
        assert_eq!(hello.publisher.as_deref(), Some("Canonical"));
        assert!(hello.verified, "the store verified Canonical");
        assert_eq!(hello.kind, Kind::App);
        assert_eq!(hello.confinement, Some(Confinement::Strict));
        assert_eq!(hello.installed_size, Some(20_480));
        assert!(hello.install_date.is_some(), "an installed snap says when");
        assert!(hello.icon_url.as_deref().is_some_and(|url| url.starts_with("https://")), "{:?}", hello.icon_url);
        let apps: Vec<&str> = snaps.iter().filter(|snap| snap.kind.is_app()).map(|snap| snap.name.as_str()).collect();
        assert_eq!(apps, ["hello-world", "test-snapd-classic-confinement", "hello"], "bases are no applications");
        let core20 = snaps.iter().find(|snap| snap.name == "core20").expect("a base came along");
        assert_eq!(core20.kind, Kind::Base);
        assert_eq!(snaps.iter().find(|snap| snap.name == "core").map(|snap| snap.kind.clone()), Some(Kind::Os));
        assert_eq!(snaps.iter().find(|snap| snap.name == "snapd").map(|snap| snap.kind.clone()), Some(Kind::Snapd));
        let classic = snaps.iter().find(|snap| snap.name == "test-snapd-classic-confinement").expect("installed");
        assert!(classic.is_classic(), "it was installed with --classic");
        assert_eq!(classic.summary, None, "the store carries none, so nothing is invented");
        assert_eq!(classic.title, "test-snapd-classic-confinement", "with no shown name the name stands");
    }

    #[test]
    fn a_search_is_read_with_its_download_sizes() {
        let (snaps, problems) = parse_snaps(&fixture("api-find-q.json")).expect("the recording reads");
        assert_eq!(problems, []);
        assert_eq!(snaps.len(), 17);
        assert_eq!(snaps[0].name, "hello-world", "the exact name leads");
        assert_eq!(snaps[0].download_size, Some(20_480));
        assert!(snaps[0].install_date.is_some(), "this one is installed here as well");
        assert!(snaps.iter().all(|snap| snap.kind.is_app()), "a search for an application finds applications");
        let (one, _) = parse_snaps(&fixture("api-find-name.json")).expect("the recording reads");
        assert_eq!(one.len(), 1, "asking by name answers with one record");
        assert_eq!(one[0].name, "hello-world");
    }

    #[test]
    fn a_record_without_a_name_is_reported_and_the_rest_read() {
        let text = "{\"result\":[{\"name\":\"hello\"},{\"title\":\"no name\"},{\"name\":\"core22\"}]}";
        let (snaps, problems) = parse_snaps(text).expect("the list reads");
        assert_eq!(snaps.iter().map(|snap| snap.name.as_str()).collect::<Vec<_>>(), ["hello", "core22"]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("snap 1"), "{}", problems[0].message);
        assert_eq!(parse_snaps("{\"result\":[]}").expect("an empty list reads").0, []);
        assert!(parse_snaps("{\"result\":{}}").is_err(), "an object is not a list of snaps");
    }

    #[test]
    fn the_categories_are_read_in_the_order_snapd_lists_them() {
        let categories = parse_categories(&fixture("api-categories.json")).expect("the recording reads");
        assert_eq!(categories.len(), 20);
        assert_eq!(categories[0], "art-and-design");
        assert!(categories.contains(&String::from("featured")));
        assert!(parse_categories("{\"result\":\"x\"}").is_err());
    }

    #[test]
    fn a_running_job_gives_the_downloads_real_share() {
        let change = parse_change(&fixture("api-change-doing.json")).expect("the recording reads");
        assert_eq!(change.id, "11");
        assert_eq!(change.kind, "install-snap");
        assert_eq!(change.summary, "Install \"core22\" snap");
        assert_eq!(change.status, "Doing");
        assert!(!change.ready);
        assert!(!change.succeeded() && !change.failed(), "nothing is claimed while it runs");
        let running = change.running().expect("one step runs");
        assert_eq!(running.kind, "download-snap");
        assert_eq!(running.label, "core22");
        assert_eq!((running.done, running.total), (487_895, 77_574_144));
        let share = running.fraction().expect("a download counts bytes");
        assert!((0.006..0.007).contains(&share), "{share}");
        let waiting = change.tasks.iter().find(|task| task.status == "Do").expect("steps are still to come");
        assert_eq!(waiting.fraction(), None, "one of one says nothing about how long it takes");
    }

    #[test]
    fn a_finished_job_says_so_and_a_users_read_of_roots_job_is_the_same_record() {
        let done = parse_change(&fixture("api-change-done.json")).expect("the recording reads");
        assert_eq!(done.status, "Done");
        assert!(done.succeeded() && !done.failed());
        assert_eq!(done.running(), None, "nothing runs any more");
        assert!(done.tasks.iter().all(|task| task.status == "Done"), "every step went well");
        let as_user = parse_change(&fixture("api-change-user.json")).expect("the recording reads");
        assert_eq!(as_user.id, "11");
        assert!(!as_user.ready, "an ordinary user reads root's job while it runs");
        assert_eq!(as_user.tasks.len(), 15);
    }

    #[test]
    fn a_job_that_is_over_without_being_done_failed() {
        let change = Change {
            id: String::from("3"),
            kind: String::from("install-snap"),
            summary: String::new(),
            status: String::from("Error"),
            ready: true,
            tasks: Vec::new(),
        };
        assert!(change.failed() && !change.succeeded());
        assert!(!Change { ready: false, ..change }.failed(), "a job still running has not failed");
    }

    #[test]
    fn a_broken_job_answer_is_no_job() {
        assert!(parse_change("{\"result\":{}}").is_err(), "no number is no job");
        assert!(parse_change("{}").is_err());
        let bare = parse_change("{\"result\":{\"id\":\"7\"}}").expect("a number is enough");
        assert_eq!(bare.tasks, []);
        assert_eq!(bare.running(), None);
        assert!(!bare.ready);
    }
}

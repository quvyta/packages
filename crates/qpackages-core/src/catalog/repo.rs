//! What the official repositories answer: `pacman -Ss` for a search, `pacman -Si` for one package.
//!
//! Both are read with `LC_ALL=C`: pacman translates its field names and the `[installed]` mark,
//! and the parsers here only know the English ones. Neither needs privileges or touches the
//! network; they read the sync databases pacman keeps.

use super::merge::RepoPackage;

/// The arguments of a repository search for `term`, program name excluded, or `None` when the
/// term is empty.
///
/// pacman reads every target of `-Ss` as a regular expression; the term is escaped so `c++` or
/// `.net` are searched for as written, and it follows `--` so a term starting with `-` is never
/// read as an option.
#[must_use]
pub fn search_args(term: &str) -> Option<Vec<String>> {
    let term = term.trim();
    if term.is_empty() {
        return None;
    }
    let mut escaped = String::with_capacity(term.len());
    for character in term.chars() {
        if "\\.^$|?*+()[]{}".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Some(vec![String::from("-Ss"), String::from("--"), escaped])
}

/// The arguments asking pacman about the repository package `name`.
#[must_use]
pub fn info_args(name: &str) -> Vec<String> {
    vec![String::from("-Si"), String::from("--"), name.to_owned()]
}

/// Reads the answer of `pacman -Ss`: a `repo/name version (groups) [installed]` line, then the
/// description indented under it. Lines that fit neither shape are skipped.
#[must_use]
pub fn parse_search(text: &str) -> Vec<RepoPackage> {
    let mut packages: Vec<RepoPackage> = Vec::new();
    for line in text.lines() {
        if let Some(description) = line.strip_prefix("    ") {
            if let Some(last) = packages.last_mut()
                && last.description.is_none()
            {
                let description = description.trim();
                last.description = (!description.is_empty()).then(|| description.to_owned());
            }
            continue;
        }
        let mut words = line.split_whitespace();
        let Some((_, name)) = words.next().and_then(|first| first.split_once('/')) else { continue };
        if name.is_empty() || words.next().is_none() {
            continue;
        }
        let rest = words.collect::<Vec<_>>().join(" ");
        let groups = rest
            .strip_prefix('(')
            .and_then(|open| open.split_once(')'))
            .map(|(inside, _)| inside.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        packages.push(RepoPackage { name: name.to_owned(), description: None, groups });
    }
    packages
}

/// One repository package as `pacman -Si` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoInfo {
    /// The repository it is in, such as `extra`.
    pub repo: String,
    /// The package name.
    pub name: String,
    /// The version, including the release.
    pub version: String,
    /// The one-line description.
    pub description: Option<String>,
    /// The project's home page.
    pub url: Option<String>,
    /// The licences.
    pub licenses: Vec<String>,
    /// The pacman groups it belongs to.
    pub groups: Vec<String>,
    /// What it needs to run.
    pub depends: Vec<String>,
    /// What it can use, each as `name: why`.
    pub optional: Vec<String>,
    /// What downloading it takes, in bytes.
    pub download_size: Option<u64>,
    /// What it takes on disk once installed, in bytes.
    pub installed_size: Option<u64>,
}

/// Reads the first package of a `pacman -Si` answer, or `None` when it has no name or version.
///
/// Fields are `Name : value` lines; a value that continues on the next lines is indented under
/// the first, which `Optional Deps` does. `None` means an empty list.
#[must_use]
pub fn parse_info(text: &str) -> Option<RepoInfo> {
    let mut info = RepoInfo::default();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            if current.is_some() {
                break;
            }
            continue;
        }
        let (key, value) = if line.starts_with(' ') {
            match &current {
                Some(key) => (key.clone(), line.trim()),
                None => continue,
            }
        } else {
            let Some((key, value)) = line.split_once(" : ").or_else(|| line.split_once(':')) else { continue };
            let key = key.trim().to_owned();
            current = Some(key.clone());
            (key, value.trim())
        };
        take(&mut info, &key, value);
    }
    (!info.name.is_empty() && !info.version.is_empty()).then_some(info)
}

/// Stores one field's value.
fn take(info: &mut RepoInfo, key: &str, value: &str) {
    let list = |value: &str| -> Vec<String> {
        if value == "None" { Vec::new() } else { value.split_whitespace().map(str::to_owned).collect() }
    };
    let text = |value: &str| (!value.is_empty() && value != "None").then(|| value.to_owned());
    match key {
        "Repository" => value.clone_into(&mut info.repo),
        "Name" => value.clone_into(&mut info.name),
        "Version" => value.clone_into(&mut info.version),
        "Description" => info.description = text(value),
        "URL" => info.url = text(value),
        "Licenses" => info.licenses = list(value),
        "Groups" => info.groups = list(value),
        "Depends On" => info.depends = list(value),
        "Optional Deps" => info.optional.extend(text(value)),
        "Download Size" => info.download_size = size(value),
        "Installed Size" => info.installed_size = size(value),
        _ => {}
    }
}

/// Bytes from a size as pacman prints it in the C locale, such as `6.69 MiB`.
fn size(value: &str) -> Option<u64> {
    let (number, unit) = value.split_once(' ')?;
    let number: f64 = number.parse().ok()?;
    let power = match unit {
        "B" => 0,
        "KiB" => 1,
        "MiB" => 2,
        "GiB" => 3,
        "TiB" => 4,
        _ => return None,
    };
    let bytes = number * 1024_f64.powi(power);
    // A size is never negative and far below 2^53, where the conversion stops being exact.
    (0.0..9e15).contains(&bytes).then(|| bytes.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../../tests/fixtures/catalog/pacman-ss-obs.txt");
    const INFO: &str = include_str!("../../tests/fixtures/catalog/pacman-si-obs-studio.txt");

    #[test]
    fn a_term_is_searched_for_as_written() {
        assert_eq!(search_args("obs"), Some(vec![String::from("-Ss"), String::from("--"), String::from("obs")]));
        assert_eq!(search_args(" c++ ").expect("a term")[2], "c\\+\\+");
        assert_eq!(search_args(".net (x)").expect("a term")[2], "\\.net \\(x\\)");
        assert_eq!(search_args("-Rns").expect("a term")[1..], ["--", "-Rns"], "never an option");
        assert_eq!(search_args("   "), None);
    }

    #[test]
    fn reads_a_recorded_search() {
        let packages = parse_search(SEARCH);
        let names: Vec<&str> = packages.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "kdav",
                "markdown-oxide",
                "obs-studio",
                "obs-studio-plugin-browser",
                "obsidian",
                "obsidian-icon-theme",
                "print-manager"
            ]
        );
        assert_eq!(
            packages[2].description.as_deref(),
            Some("Free, open source software for live streaming and recording")
        );
        assert_eq!(packages[0].groups, ["kf6"]);
        assert_eq!(packages[6].groups, ["plasma", "kde-system"], "several groups, then the installed mark");
        assert!(packages[4].groups.is_empty(), "an installed mark with a version is not a group");
    }

    #[test]
    fn nothing_found_is_an_empty_list() {
        assert!(parse_search("").is_empty());
        assert!(parse_search("error: something\n    stray\n").is_empty());
    }

    #[test]
    fn reads_a_recorded_package() {
        let info = parse_info(INFO).expect("a package");
        assert_eq!(info.repo, "extra");
        assert_eq!(info.name, "obs-studio");
        assert_eq!(info.version, "32.2.2-1");
        assert_eq!(info.url.as_deref(), Some("https://obsproject.com"));
        assert_eq!(info.licenses, ["GPL-2.0-only"]);
        assert!(info.groups.is_empty(), "None is an empty list");
        assert_eq!(info.depends.len(), 19);
        assert_eq!(info.depends[0], "ffmpeg");
        assert_eq!(info.optional.len(), 8, "the continued lines belong to Optional Deps");
        assert_eq!(info.optional[7], "obs-studio-plugin-browser: CEF-based browser plugin");
        assert_eq!(info.download_size, Some(7_014_973));
        assert_eq!(info.installed_size, Some(25_910_313));
    }

    #[test]
    fn only_the_first_package_is_read_and_a_broken_answer_is_none() {
        let two = format!("{INFO}Repository      : core\nName            : other\nVersion         : 1-1\n");
        assert_eq!(parse_info(&two).expect("a package").name, "obs-studio");
        assert_eq!(parse_info(""), None);
        assert_eq!(parse_info("Name : x\n"), None, "no version");
        assert_eq!(
            parse_info("   stray continuation\nName : x\nVersion : 1\n").map(|info| info.name),
            Some(String::from("x"))
        );
    }

    #[test]
    fn sizes_come_in_every_unit_and_nonsense_is_none() {
        assert_eq!(size("512 B"), Some(512));
        assert_eq!(size("1.50 KiB"), Some(1536));
        assert_eq!(size("2.00 GiB"), Some(2_147_483_648));
        assert_eq!(size("2 parsecs"), None);
        assert_eq!(size("-1 KiB"), None);
        assert_eq!(size("NaN MiB"), None);
    }
}

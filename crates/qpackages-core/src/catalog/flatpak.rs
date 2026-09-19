//! Which Flatpak applications are installed, from `flatpak list`.
//!
//! Listing reads Flatpak's installations and changes nothing, so it runs as the user. Each line
//! of the answer is an application id and the installation holding it, separated by a tab;
//! without a terminal Flatpak prints no header. An application can be installed both for the
//! user and for the whole system, and then has a line for each.

use super::Problem;

/// The program to run.
pub const FLATPAK: &str = "flatpak";

/// Where an application is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installation {
    /// For this user only (`--user`).
    User,
    /// For every user of the machine (`--system`).
    System,
    /// An extra system installation configured by name.
    Named(String),
}

impl Installation {
    fn from_column(column: &str) -> Self {
        match column {
            "user" => Self::User,
            "system" => Self::System,
            name => Self::Named(name.to_owned()),
        }
    }
}

/// One installed application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// The application id, the same as its AppStream id.
    pub app_id: String,
    /// Where it is installed.
    pub installation: Installation,
}

/// The arguments listing the installed applications (runtimes left out), program name excluded.
#[must_use]
pub fn list_args() -> Vec<String> {
    ["list", "--app", "--columns=application,installation"].map(str::to_owned).to_vec()
}

/// Reads what [`list_args`] prints. An empty answer is no applications. A line that is not an
/// id and an installation, such as a header a terminal would get, is skipped and reported.
#[must_use]
pub fn parse_list(text: &str) -> (Vec<InstalledApp>, Vec<Problem>) {
    let mut apps = Vec::new();
    let mut problems = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            continue;
        }
        let mut columns = line.split('\t').map(str::trim);
        let (Some(app_id), Some(installation)) = (columns.next(), columns.next()) else {
            problems.push(Problem::at(text, start, "expected an application id and an installation"));
            continue;
        };
        // Application ids are reverse domain names: at least one dot, no spaces.
        if !app_id.contains('.') || app_id.contains(' ') || installation.is_empty() {
            problems.push(Problem::at(text, start, format!("`{app_id}` is not an application id")));
            continue;
        }
        apps.push(InstalledApp { app_id: app_id.to_owned(), installation: Installation::from_column(installation) });
    }
    (apps, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("../../tests/fixtures/catalog/flatpak-list-apps.txt");

    #[test]
    fn the_arguments_list_applications_only_with_their_installation() {
        assert_eq!(list_args(), ["list", "--app", "--columns=application,installation"]);
    }

    #[test]
    fn reads_a_recorded_list() {
        let (apps, problems) = parse_list(LIST);
        assert_eq!(problems, []);
        assert_eq!(apps.len(), 4);
        assert_eq!(
            apps[0],
            InstalledApp { app_id: String::from("com.spotify.Client"), installation: Installation::System }
        );
        assert!(apps.iter().any(|app| app.app_id == "org.videolan.VLC" && app.installation == Installation::User));
        assert_eq!(apps.iter().filter(|app| app.app_id == "org.gimp.GIMP").count(), 2, "one line per installation");
    }

    #[test]
    fn nothing_installed_is_an_empty_answer() {
        assert_eq!(parse_list(""), (Vec::new(), Vec::new()));
        assert_eq!(parse_list("\n"), (Vec::new(), Vec::new()));
    }

    #[test]
    fn a_header_or_a_broken_line_is_reported_and_the_rest_read() {
        let text = "Application ID\tInstallation\norg.gnome.Maps\tsystem\njunk\na.b.C\tsteamdeck\n";
        let (apps, problems) = parse_list(text);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[1].installation, Installation::Named(String::from("steamdeck")));
        let lines: Vec<usize> = problems.iter().map(|problem| problem.line).collect();
        assert_eq!(lines, [1, 3]);
    }
}

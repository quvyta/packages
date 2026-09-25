//! What qpackages asks Flatpak to do: the argument lists that install and remove applications,
//! the ones that add Flathub and look for updates, and reading which remotes the user has.
//!
//! Installing and removing for the user needs no privileges and runs as the user. Only removing
//! an application installed for the whole system goes through the root helper, which checks
//! every id with [`is_app_id`] before it runs anything. The argument lists put `--` before the
//! ids, so an id can never be read as an option; measured with Flatpak 1.18.2.

/// Flatpak by its absolute path, for the root helper, which never looks a program up.
pub const FLATPAK_PATH: &str = "/usr/bin/flatpak";

/// The name the Flathub remote is added under, the one every guide uses.
pub const FLATHUB: &str = "flathub";

/// Where Flathub's remote description is.
pub const FLATHUB_REPO: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// The longest application id accepted, in bytes: Flatpak's own limit for a name.
const MAX_ID: usize = 255;

/// Whether `id` is an application id the helper may hand to Flatpak: a reverse domain name of at
/// least three parts, each non-empty and made of ASCII letters, digits, `_` and `-`, not starting
/// with `-`, at most 255 bytes.
#[must_use]
pub fn is_app_id(id: &str) -> bool {
    let part =
        |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    id.len() <= MAX_ID && !id.starts_with('-') && id.split('.').count() >= 3 && id.split('.').all(part)
}

fn owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// Installs `ids` from Flathub for this user, without questions, program name excluded.
#[must_use]
pub fn install_args(ids: &[String]) -> Vec<String> {
    let mut args = owned(&["--user", "install", "--noninteractive", "--", FLATHUB]);
    args.extend_from_slice(ids);
    args
}

/// Removes `ids` installed for this user, program name excluded.
#[must_use]
pub fn uninstall_args(ids: &[String]) -> Vec<String> {
    let mut args = owned(&["--user", "uninstall", "--noninteractive", "--"]);
    args.extend_from_slice(ids);
    args
}

/// Removes the runtimes and extensions of this user that no application uses any more.
#[must_use]
pub fn uninstall_unused_args() -> Vec<String> {
    owned(&["--user", "uninstall", "--unused", "--noninteractive"])
}

/// Removes `ids` installed for the whole system; only the root helper runs it.
#[must_use]
pub fn system_uninstall_args(ids: &[String]) -> Vec<String> {
    let mut args = owned(&["--system", "uninstall", "--noninteractive", "--"]);
    args.extend_from_slice(ids);
    args
}

/// Lists this user's remotes by name and address; without a terminal Flatpak prints no header.
#[must_use]
pub fn remotes_args() -> Vec<String> {
    owned(&["remotes", "--user", "--columns=name,url"])
}

/// Adds Flathub for this user; nothing happens when it is already there.
#[must_use]
pub fn add_flathub_args() -> Vec<String> {
    owned(&["remote-add", "--user", "--if-not-exists", FLATHUB, FLATHUB_REPO])
}

/// Whether what [`remotes_args`] printed names Flathub. No remotes at all is a single empty line.
#[must_use]
pub fn has_flathub(text: &str) -> bool {
    text.lines().filter_map(|line| line.split('\t').next()).any(|name| name.trim() == FLATHUB)
}

/// Which Flatpak installation an update query reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The calling user's installation.
    User,
    /// The installation shared by the machine.
    System,
}

impl Scope {
    const fn option(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::System => "--system",
        }
    }
}

/// Lists the refs in `scope` that have an update waiting; program name excluded.
#[must_use]
pub fn updates_args(scope: Scope) -> Vec<String> {
    owned(&[scope.option(), "remote-ls", "--updates", "--columns=application,branch,version,commit"])
}

/// Lists the refs installed in `scope` with the version Flatpak records for each; program name
/// excluded.
#[must_use]
pub fn installed_args(scope: Scope) -> Vec<String> {
    owned(&[scope.option(), "list", "--columns=application,version,branch,installation"])
}

/// What a pair of Flatpak update queries meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The query ran; the list is empty when that installation has nothing waiting.
    Updates(Vec<crate::pacman::Update>),
    /// A command did not succeed; what it said, when it said anything.
    Failed {
        /// The command's error stream, or its standard output when that is all it wrote.
        stderr: String,
    },
    /// A successful answer did not have the tab-separated shape Flatpak documents.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledRef {
    name: String,
    branch: String,
    version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WaitingRef {
    name: String,
    branch: String,
    version: String,
    commit: String,
}

/// Reads the output of [`updates_args`] beside the matching [`installed_args`] output.
///
/// Flatpak often gives a runtime no release version. Its branch is a true label for what is
/// installed, while the first twelve characters of the waiting commit distinguish the new ref;
/// together they say what changed without inventing a version Flatpak did not provide.
#[must_use]
pub fn read_updates(
    updates_code: Option<i32>,
    updates_stdout: &str,
    updates_stderr: &str,
    installed_code: Option<i32>,
    installed_stdout: &str,
    installed_stderr: &str,
) -> UpdateCheck {
    if updates_code != Some(0) {
        return UpdateCheck::Failed { stderr: first_line(updates_stderr, updates_stdout) };
    }
    if installed_code != Some(0) {
        return UpdateCheck::Failed { stderr: first_line(installed_stderr, installed_stdout) };
    }
    let Some(waiting) = parse_waiting(updates_stdout) else { return UpdateCheck::Unreadable };
    if waiting.is_empty() {
        return UpdateCheck::Updates(Vec::new());
    }
    let Some(installed) = parse_installed(installed_stdout) else { return UpdateCheck::Unreadable };
    let updates = waiting
        .into_iter()
        .map(|remote| {
            let current = installed.iter().find(|item| item.name == remote.name && item.branch == remote.branch)?;
            let from = if current.version.is_empty() { &current.branch } else { &current.version };
            let to = if remote.version.is_empty() {
                remote.commit.get(..12).unwrap_or(&remote.commit)
            } else {
                &remote.version
            };
            Some(crate::pacman::Update { name: remote.name, from: from.to_owned(), to: to.to_owned(), ignored: false })
        })
        .collect::<Option<Vec<_>>>();
    match updates {
        Some(updates) => UpdateCheck::Updates(updates),
        None => UpdateCheck::Unreadable,
    }
}

fn parse_waiting(text: &str) -> Option<Vec<WaitingRef>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split('\t').map(str::trim);
            let name = columns.next()?;
            let branch = columns.next()?;
            let version = columns.next()?;
            let commit = columns.next()?;
            if columns.next().is_some()
                || name.is_empty()
                || branch.is_empty()
                || (version.is_empty() && commit.is_empty())
            {
                return None;
            }
            Some(WaitingRef {
                name: name.to_owned(),
                branch: branch.to_owned(),
                version: version.to_owned(),
                commit: commit.to_owned(),
            })
        })
        .collect()
}

fn parse_installed(text: &str) -> Option<Vec<InstalledRef>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split('\t').map(str::trim);
            let name = columns.next()?;
            let version = columns.next()?;
            let branch = columns.next()?;
            let _installation = columns.next()?;
            if columns.next().is_some() || name.is_empty() || branch.is_empty() {
                return None;
            }
            Some(InstalledRef { name: name.to_owned(), branch: branch.to_owned(), version: version.to_owned() })
        })
        .collect()
}

fn first_line(stderr: &str, stdout: &str) -> String {
    stderr.lines().chain(stdout.lines()).map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flatpak").join(name);
        std::fs::read_to_string(path).expect("the fixture is readable")
    }

    #[test]
    fn real_ids_pass_the_rule() {
        for id in [
            "org.mozilla.firefox",
            "net.sourceforge.ExtremeTuxRacer",
            "io.github.celluloid_player.Celluloid",
            "org.freedesktop.Platform.GL.default",
            "com.github.tenderowl.frog",
            "io.github.nokse22.asciidraw",
            "org.example.a-b",
        ] {
            assert!(is_app_id(id), "`{id}`");
        }
        assert!(is_app_id(&format!("a.b.{}", "c".repeat(MAX_ID - 4))), "the longest id");
    }

    #[test]
    fn ids_breaking_the_rule_fail_it() {
        for id in [
            "",
            "firefox",
            "org.firefox",
            "-org.mozilla.firefox",
            "--system",
            "org..firefox",
            ".org.mozilla.firefox",
            "org.mozilla.firefox.",
            "org.mozilla.fire fox",
            "org.mozilla.fire/fox",
            "org.mozilla.fırefox",
            "org.mozilla.firefox\t",
            "org.mozilla.firefox//stable",
            "app/org.mozilla.firefox/x86_64/stable",
            "../../etc.passwd.x",
        ] {
            assert!(!is_app_id(id), "`{id}`");
        }
        assert!(!is_app_id(&format!("a.b.{}", "c".repeat(MAX_ID - 3))), "one byte too long");
    }

    #[test]
    fn the_argument_lists_are_the_measured_ones() {
        let ids = owned(&["net.sourceforge.ExtremeTuxRacer"]);
        assert_eq!(
            install_args(&ids),
            ["--user", "install", "--noninteractive", "--", "flathub", "net.sourceforge.ExtremeTuxRacer"]
        );
        assert_eq!(
            uninstall_args(&ids),
            ["--user", "uninstall", "--noninteractive", "--", "net.sourceforge.ExtremeTuxRacer"]
        );
        assert_eq!(uninstall_unused_args(), ["--user", "uninstall", "--unused", "--noninteractive"]);
        assert_eq!(
            system_uninstall_args(&ids),
            ["--system", "uninstall", "--noninteractive", "--", "net.sourceforge.ExtremeTuxRacer"]
        );
        assert_eq!(remotes_args(), ["remotes", "--user", "--columns=name,url"]);
        assert_eq!(
            add_flathub_args(),
            ["remote-add", "--user", "--if-not-exists", "flathub", "https://dl.flathub.org/repo/flathub.flatpakrepo"]
        );
        assert_eq!(
            updates_args(Scope::User),
            ["--user", "remote-ls", "--updates", "--columns=application,branch,version,commit"]
        );
        assert_eq!(
            updates_args(Scope::System),
            ["--system", "remote-ls", "--updates", "--columns=application,branch,version,commit"]
        );
        assert_eq!(
            installed_args(Scope::User),
            ["--user", "list", "--columns=application,version,branch,installation"]
        );
        assert_eq!(
            installed_args(Scope::System),
            ["--system", "list", "--columns=application,version,branch,installation"]
        );
    }

    #[test]
    fn flathub_is_read_from_the_recorded_remotes() {
        assert!(has_flathub(&fixture("remotes-user.txt")));
        assert!(!has_flathub(&fixture("remotes-user-empty.txt")), "no remotes is one empty line");
        assert!(!has_flathub(""));
        assert!(!has_flathub("flathub-beta\thttps://dl.flathub.org/beta-repo/\n"), "another remote is not Flathub");
        assert!(has_flathub("fedora\toci+https://registry.fedoraproject.org\nflathub\thttps://dl.flathub.org/repo/\n"));
    }

    #[test]
    fn a_recorded_update_pairs_the_new_ref_with_the_one_installed() {
        // Recorded in the throwaway Arch container with Flatpak 1.18.3.
        let result = read_updates(Some(0), &fixture("updates-user.out"), "", Some(0), &fixture("list-user.out"), "");
        let UpdateCheck::Updates(updates) = result else { panic!("the recordings are read") };
        assert_eq!(
            updates,
            [
                crate::pacman::Update {
                    name: "net.sourceforge.ExtremeTuxRacer".to_owned(),
                    from: "0.8.4".to_owned(),
                    to: "8f72400b6553".to_owned(),
                    ignored: false,
                },
                crate::pacman::Update {
                    name: "org.freedesktop.Platform".to_owned(),
                    from: "freedesktop-sdk-25.08.16".to_owned(),
                    to: "d27f7a6a974e".to_owned(),
                    ignored: false,
                },
            ]
        );
    }

    #[test]
    fn a_runtime_without_a_version_names_its_branch_and_waiting_commit() {
        // codecs-extra is recorded in the throwaway Arch container with no version; the waiting
        // line is written by hand, since that container had no newer commit for it.
        let waiting = "org.freedesktop.Platform.codecs-extra\t25.08-extra\t\t0123456789abcdef0123\n";
        let UpdateCheck::Updates(updates) = read_updates(Some(0), waiting, "", Some(0), &fixture("list-user.out"), "")
        else {
            panic!("the recordings are read")
        };
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].from, "25.08-extra", "the branch stands in for Flatpak's empty version");
        assert_eq!(updates[0].to, "0123456789ab", "twelve characters of the new commit");
    }

    #[test]
    fn an_installation_with_nothing_waiting_is_an_empty_answer() {
        // Recorded in the throwaway Arch container with Flatpak 1.18.3.
        for (updates, installed) in [
            ("updates-user-empty.out", "list-user.out"),
            ("no-remote-user.out", "list-user.out"),
            ("no-installation-system.out", "list-system.out"),
        ] {
            assert_eq!(
                read_updates(Some(0), &fixture(updates), "", Some(0), &fixture(installed), ""),
                UpdateCheck::Updates(Vec::new()),
                "{updates}"
            );
        }
    }

    #[test]
    fn a_recorded_flatpak_error_is_not_an_empty_update_list() {
        // Recorded in the throwaway Arch container with Flatpak 1.18.3.
        assert_eq!(
            read_updates(Some(1), "", &fixture("updates-error.err"), Some(0), &fixture("list-user.out"), "",),
            UpdateCheck::Failed {
                stderr: "error: Remote \"no-such-remote\" not found in the user installation".to_owned()
            }
        );
    }

    #[test]
    fn output_of_another_shape_is_unreadable() {
        assert_eq!(
            read_updates(Some(0), "application\tstable\n", "", Some(0), "id\t1\tstable\tuser\n", ""),
            UpdateCheck::Unreadable
        );
    }
}

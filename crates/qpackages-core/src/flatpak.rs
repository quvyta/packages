//! What qpackages asks Flatpak to do: the argument lists that install and remove applications,
//! the one that adds Flathub, and reading which remotes the user has.
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
    }

    #[test]
    fn flathub_is_read_from_the_recorded_remotes() {
        assert!(has_flathub(&fixture("remotes-user.txt")));
        assert!(!has_flathub(&fixture("remotes-user-empty.txt")), "no remotes is one empty line");
        assert!(!has_flathub(""));
        assert!(!has_flathub("flathub-beta\thttps://dl.flathub.org/beta-repo/\n"), "another remote is not Flathub");
        assert!(has_flathub("fedora\toci+https://registry.fedoraproject.org\nflathub\thttps://dl.flathub.org/repo/\n"));
    }
}

//! The exact argument lists qpackages hands to pacman and to the programs around it.
//!
//! Every function returns the arguments after the program name and nothing else, so one place
//! decides the flags and the caller decides how the program is reached: the root helper's
//! `/usr/bin/pacman …` for a transaction, plain `pacman …` for a query that needs no privileges,
//! `fakeroot -- pacman …` for the private refresh. Pinning the lists here keeps them testable
//! without running anything.

use std::path::Path;

/// The package manager.
pub const PACMAN: &str = "pacman";
/// Lets pacman believe it is root while it only touches a private database copy.
pub const FAKEROOT: &str = "fakeroot";
/// How privileges are obtained for a transaction. Its own prompt asks for the password.
pub const SUDO: &str = "sudo";

/// The machine format asked of `pacman -S --print`: repository, name, version and download size.
///
/// `|` separates the fields only inside this format; it is never drawn on screen.
const INSTALL_FORMAT: &str = "%r|%n|%v|%s";

/// The machine format asked of `pacman -Rns --print`: name and version.
///
/// `|` separates the fields only inside this format; it is never drawn on screen.
const REMOVE_FORMAT: &str = "%n|%v";

/// The arguments that install `names`: `-S --needed --noconfirm`.
///
/// `--noconfirm` because the user has already confirmed on our screen and pacman's own
/// question would be asked on a pseudo-terminal nobody types into. `--needed` so a package
/// that is already at the requested version is left alone instead of reinstalled.
#[must_use]
pub fn install(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-S", "--needed", "--noconfirm"], names)
}

/// The arguments that remove `names` with the dependencies only they needed and the
/// configuration files pacman would otherwise keep as `.pacsave`: `-Rns --noconfirm`.
#[must_use]
pub fn remove(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-Rns", "--noconfirm"], names)
}

/// The arguments that bring the whole system up to date: `-Syu --noconfirm`.
///
/// The repositories are refreshed and everything is upgraded in the same transaction, the only
/// way a refresh never leaves a partial upgrade behind.
#[must_use]
pub fn upgrade() -> Vec<String> {
    ["-Syu", "--noconfirm"].map(str::to_owned).to_vec()
}

/// The arguments that bring the system up to date and install `names` in the same transaction:
/// `-Syu --needed --noconfirm`. Installing against a system with pending updates would pull in
/// libraries newer than the rest of it was built against.
#[must_use]
pub fn upgrade_install(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-Syu", "--needed", "--noconfirm"], names)
}

/// The arguments that mark `names` as installed only as dependencies: `-D --asdeps`. Once no
/// package needs them they count as orphans.
#[must_use]
pub fn mark_deps(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-D", "--asdeps"], names)
}

/// The arguments that mark `names` as installed on purpose: `-D --asexplicit`, so an orphan the
/// user chose to keep is never offered for removal again.
#[must_use]
pub fn mark_explicit(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-D", "--asexplicit"], names)
}

/// The arguments that list the orphans, one name per line: installed as dependencies and needed
/// by nothing any more (`-Qtdq`). Read by [`read_orphans`](super::read_orphans).
#[must_use]
pub fn orphans() -> Vec<String> {
    vec!["-Qtdq".to_owned()]
}

/// The arguments that list the packages no repository knows, `name version` per line (`-Qm`):
/// on Arch these came from the AUR or were built by hand.
#[must_use]
pub fn foreign_versions() -> Vec<String> {
    vec!["-Qm".to_owned()]
}

/// The arguments that print what installing `names` would do, without doing it.
///
/// The output is one `repo|name|version|size` line per package, read by
/// [`parse_install_plan`](super::parse_install_plan).
#[must_use]
pub fn print_install(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-S", "--print", "--print-format", INSTALL_FORMAT], names)
}

/// The arguments that print what removing `names` would do, without doing it.
///
/// The output is one `name|version` line per package, read by
/// [`parse_remove_plan`](super::parse_remove_plan).
#[must_use]
pub fn print_remove(names: &[impl AsRef<str>]) -> Vec<String> {
    with_names(["-Rns", "--print", "--print-format", REMOVE_FORMAT], names)
}

/// The arguments that list pending updates against the database under `dbpath`.
///
/// `dbpath` is the private copy from [`syncdb::prepare`](super::syncdb::prepare), never the
/// system database, so the check needs no privileges. The output is read by
/// [`read_update_check`](super::read_update_check).
#[must_use]
pub fn update_check(dbpath: &Path) -> Vec<String> {
    vec!["-Qu".to_owned(), "--dbpath".to_owned(), dbpath.to_string_lossy().into_owned()]
}

/// The arguments that list the installed packages no repository offers, one name per line:
/// `-Qqm`. On Arch these are the ones built from the AUR (or by hand); the check reads the system
/// database without changing it, so it needs no privileges.
#[must_use]
pub fn foreign() -> Vec<String> {
    vec!["-Qqm".to_owned()]
}

/// The arguments for an AUR helper (`paru` or `yay`) that list the AUR packages with a newer
/// version: `-Qua`. The output has the shape of `pacman -Qu` and is read by
/// [`read_update_check`](super::read_update_check).
#[must_use]
pub fn aur_update_check() -> Vec<String> {
    vec!["-Qua".to_owned()]
}

/// The arguments for [`FAKEROOT`] that refresh the private database copy under `dbpath`.
///
/// This is the only place `-Sy` appears, and it is aimed at a copy: refreshing the system
/// database without upgrading invites a partial upgrade. pacman insists on being root to sync,
/// so fakeroot lends it the appearance; the log goes to `/dev/null` because the real log is
/// root's, and the download sandbox is off because it needs a real root to switch users.
#[must_use]
pub fn refresh(dbpath: &Path) -> Vec<String> {
    vec![
        "--".to_owned(),
        PACMAN.to_owned(),
        "-Sy".to_owned(),
        "--dbpath".to_owned(),
        dbpath.to_string_lossy().into_owned(),
        "--logfile".to_owned(),
        "/dev/null".to_owned(),
        "--disable-sandbox".to_owned(),
    ]
}

/// The arguments for [`SUDO`] that warm its ticket by asking for the password and nothing more,
/// so the root helper can be started right after without a prompt.
#[must_use]
pub fn warm_ticket() -> Vec<String> {
    vec!["-v".to_owned()]
}

/// The environment for every call whose output is parsed.
///
/// pacman honours `LC_ALL`, but flatpak reads only `LANG`; both are pinned to the C locale so no
/// tool answers in a translated form that a parser would have to guess at.
#[must_use]
pub fn parsed_env() -> [(&'static str, &'static str); 2] {
    [("LC_ALL", "C"), ("LANG", "C")]
}

/// The fixed flags, `--`, then the package names: after `--` pacman reads nothing as a flag, so
/// no name can ever turn into one.
fn with_names<const N: usize>(flags: [&str; N], names: &[impl AsRef<str>]) -> Vec<String> {
    flags
        .iter()
        .chain(std::iter::once(&"--"))
        .map(|flag| (*flag).to_owned())
        .chain(names.iter().map(|name| name.as_ref().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_confirms_nothing_and_skips_what_is_already_there() {
        assert_eq!(install(&["gimp", "bash"]), ["-S", "--needed", "--noconfirm", "--", "gimp", "bash"]);
    }

    #[test]
    fn remove_takes_the_dependencies_only_the_package_needed_and_its_saved_settings() {
        assert_eq!(remove(&["yay"]), ["-Rns", "--noconfirm", "--", "yay"]);
    }

    #[test]
    fn upgrades_refresh_and_upgrade_in_one_transaction() {
        assert_eq!(upgrade(), ["-Syu", "--noconfirm"]);
        assert_eq!(upgrade_install(&["gimp"]), ["-Syu", "--needed", "--noconfirm", "--", "gimp"]);
    }

    #[test]
    fn marks_and_orphan_queries_carry_their_fixed_flags() {
        assert_eq!(mark_deps(&["gimp"]), ["-D", "--asdeps", "--", "gimp"]);
        assert_eq!(mark_explicit(&["gimp", "vlc"]), ["-D", "--asexplicit", "--", "gimp", "vlc"]);
        assert_eq!(orphans(), ["-Qtdq"]);
        assert_eq!(foreign_versions(), ["-Qm"]);
    }

    #[test]
    fn print_install_asks_for_the_machine_format_the_plan_reader_expects() {
        assert_eq!(print_install(&["gimp"]), ["-S", "--print", "--print-format", "%r|%n|%v|%s", "--", "gimp"]);
    }

    #[test]
    fn print_remove_plans_with_the_same_flags_as_the_removal() {
        assert_eq!(print_remove(&["yay"]), ["-Rns", "--print", "--print-format", "%n|%v", "--", "yay"]);
    }

    #[test]
    fn an_empty_name_list_still_carries_the_fixed_flags() {
        let none: [&str; 0] = [];
        assert_eq!(install(&none), ["-S", "--needed", "--noconfirm", "--"]);
        assert_eq!(remove(&none), ["-Rns", "--noconfirm", "--"]);
        assert_eq!(print_install(&none), ["-S", "--print", "--print-format", "%r|%n|%v|%s", "--"]);
        assert_eq!(print_remove(&none), ["-Rns", "--print", "--print-format", "%n|%v", "--"]);
    }

    #[test]
    fn owned_strings_are_accepted_as_names() {
        let names = vec!["gimp".to_owned()];
        assert_eq!(install(&names), ["-S", "--needed", "--noconfirm", "--", "gimp"]);
    }

    #[test]
    fn the_update_check_points_at_the_private_database() {
        assert_eq!(update_check(Path::new("/tmp/qpackages/db")), ["-Qu", "--dbpath", "/tmp/qpackages/db"]);
    }

    #[test]
    fn the_read_only_queries_carry_only_their_query_flags() {
        assert_eq!(foreign(), ["-Qqm"]);
        assert_eq!(aur_update_check(), ["-Qua"]);
    }

    #[test]
    fn the_refresh_runs_pacman_under_fakeroot_against_the_private_database_only() {
        assert_eq!(
            refresh(Path::new("/tmp/qpackages/db")),
            ["--", "pacman", "-Sy", "--dbpath", "/tmp/qpackages/db", "--logfile", "/dev/null", "--disable-sandbox"]
        );
    }

    #[test]
    fn the_ticket_is_warmed_without_running_anything() {
        assert_eq!(warm_ticket(), ["-v"]);
    }

    #[test]
    fn parsed_calls_pin_both_locale_variables() {
        assert_eq!(parsed_env(), [("LC_ALL", "C"), ("LANG", "C")]);
    }

    #[test]
    fn the_program_names_are_the_plain_commands() {
        assert_eq!(PACMAN, "pacman");
        assert_eq!(FAKEROOT, "fakeroot");
        assert_eq!(SUDO, "sudo");
    }
}

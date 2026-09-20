//! The paru and yay command lines that build AUR packages with qpac as their sudo.
//!
//! Both are asked for AUR targets only (`--aur`), so a refresh or a system update never starts
//! from here. Their own recipe review and menus are turned off: qpac reviews the recipes before
//! the user confirms, and nobody types into the pseudo-terminal they run on. The build
//! dependencies they install are removed again afterwards (`--removemake`); the confirmation
//! says so.
//!
//! paru's `--nosudoloop` overrides a `SudoLoop` in the user's own `paru.conf`: the loop would ask
//! the shim to keep a permission warm that the helper already holds. yay has no such flag; its
//! loop is off unless the user turned it on, and then it calls qpac with a bare `-v`, without the
//! shim's flag, which qpac answers at once ([`super::shim::is_bare_validate`]).

use std::path::Path;

use super::shim::FLAG;
use crate::sources::AurHelper;

/// The arguments that have `helper` build and install `targets`, calling `qpac` for every root
/// step with the shim's flag and `folder`. `None` when `folder` has whitespace in its path: paru
/// and yay split `--sudoflags` at whitespace, so the shim would get another folder.
#[must_use]
pub fn build_args(helper: AurHelper, qpac: &Path, folder: &Path, targets: &[String]) -> Option<Vec<String>> {
    let folder = folder.to_str().filter(|path| !path.is_empty() && !path.contains(char::is_whitespace))?;
    let sudo = qpac.to_str()?;
    let own: &[&str] = match helper {
        AurHelper::Paru => &["-S", "--aur", "--skipreview", "--noconfirm", "--removemake", "--nosudoloop"],
        AurHelper::Yay => &[
            "-S",
            "--aur",
            "--answerdiff",
            "None",
            "--answerclean",
            "None",
            "--answeredit",
            "None",
            "--noconfirm",
            "--removemake",
        ],
    };
    let flags = format!("{FLAG} {folder}");
    let mut args: Vec<String> = own.iter().map(|arg| (*arg).to_owned()).collect();
    args.extend(["--sudo".to_owned(), sudo.to_owned(), "--sudoflags".to_owned(), flags, "--".to_owned()]);
    args.extend(targets.iter().cloned());
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(helper: AurHelper, folder: &str) -> Option<String> {
        build_args(helper, Path::new("/usr/bin/qpac"), Path::new(folder), &["hello".to_owned()])
            .map(|args| args.join(" "))
    }

    #[test]
    fn paru_and_yay_get_qpac_as_their_sudo_and_the_folder_after_the_flag() {
        assert_eq!(
            line(AurHelper::Paru, "/run/user/1000/quvyta-packages/3f9a").as_deref(),
            Some(
                "-S --aur --skipreview --noconfirm --removemake --nosudoloop --sudo /usr/bin/qpac \
                 --sudoflags --elevate-shim /run/user/1000/quvyta-packages/3f9a -- hello"
            )
        );
        assert_eq!(
            line(AurHelper::Yay, "/run/user/1000/quvyta-packages/3f9a").as_deref(),
            Some(
                "-S --aur --answerdiff None --answerclean None --answeredit None --noconfirm --removemake \
                 --sudo /usr/bin/qpac --sudoflags --elevate-shim /run/user/1000/quvyta-packages/3f9a -- hello"
            )
        );
    }

    #[test]
    fn the_flags_value_is_one_argument_that_splits_into_two() {
        let args = build_args(AurHelper::Paru, Path::new("/usr/bin/qpac"), Path::new("/run/x"), &[]).expect("args");
        let at = args.iter().position(|arg| arg == "--sudoflags").expect("--sudoflags");
        assert_eq!(args[at + 1].split_whitespace().collect::<Vec<_>>(), ["--elevate-shim", "/run/x"]);
    }

    #[test]
    fn a_folder_with_whitespace_is_not_used() {
        assert_eq!(line(AurHelper::Paru, "/run/user/1000/a b"), None);
        assert_eq!(line(AurHelper::Yay, "/tmp/a\tb"), None);
        assert_eq!(line(AurHelper::Yay, ""), None);
    }
}

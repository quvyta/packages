//! The update ladder: what the background check does with the updates it finds.
//!
//! Three steps, each doing what the one below it does and more: tell, download ahead, install.
//! What each one can honestly do was measured in a throwaway Arch container:
//!
//! - **Tell** is the check as it always was.
//! - **Download** fetches the repositories' pending updates as the user, without root, into a
//!   folder of the user's own; the next confirmed update copies them into pacman's cache through
//!   the helper. The AUR, Flatpak and Snap are only told about: building AUR packages ahead runs
//!   recipes nobody has reviewed yet, qpac does not update Flatpak applications, and snapd
//!   refreshes its snaps by itself.
//! - **Install** runs a root-owned script that takes no arguments, through a sudoers line the
//!   person adds themself. qpac never writes that line and never ships the script from `cargo
//!   install`, so without the script this step is shown with its reason and cannot be chosen.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The script the install step runs as root. A distribution package brings it; it takes no
/// arguments, so a sudoers line allowing exactly it allows nothing else.
pub const SCRIPT: &str = "/usr/lib/quvyta-packages/upgrade";

/// The user id that must own the script and every folder above it.
pub const ROOT: u32 = 0;

/// A step of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// The check tells what it found; nothing is downloaded or installed.
    Notify,
    /// The repositories' updates are also downloaded ahead, without root.
    Download,
    /// The repositories' updates are also installed, through the root-owned script.
    Install,
}

impl Mode {
    /// Every step, lowest first, with the value the settings file keeps.
    pub const ALL: [(Self, &'static str); 3] =
        [(Self::Notify, "notify"), (Self::Download, "download"), (Self::Install, "install")];

    /// The value the settings file keeps.
    #[must_use]
    pub fn key(self) -> &'static str {
        Self::ALL.iter().find(|(mode, _)| *mode == self).map_or("notify", |(_, key)| key)
    }

    /// The step `key` names, when it names one.
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, known)| *known == key).map(|(mode, _)| *mode)
    }

    /// What this step really does on a machine where the script is there (`script`): install is
    /// download without it, since the step below is all that can run.
    #[must_use]
    pub fn effective(self, script: bool) -> Self {
        if self == Self::Install && !script { Self::Download } else { self }
    }
}

/// The steps that can be chosen on a machine where the script is there (`script`), lowest first.
/// The Settings page and the wizard offer exactly these.
#[must_use]
pub fn offered(script: bool) -> Vec<Mode> {
    Mode::ALL.iter().map(|(mode, _)| *mode).filter(|mode| *mode != Mode::Install || script).collect()
}

/// Whether the file at `path`, under the file system's `root`, can be run as root without
/// handing root to whoever can change it: a regular file, not a link, that can run, owned by
/// `owner` (root on a real machine) and writable by nobody else, in folders that are the same all
/// the way up to `root`. A folder anyone else could write would let them put another file in its
/// place.
#[must_use]
pub fn trusted(root: &Path, path: &Path, owner: u32) -> bool {
    let safe = |meta: &fs::Metadata| meta.uid() == owner && meta.mode() & 0o022 == 0;
    let Ok(file) = fs::symlink_metadata(path) else { return false };
    if !path.starts_with(root) || !file.file_type().is_file() || !safe(&file) || file.mode() & 0o100 == 0 {
        return false;
    }
    path.ancestors()
        .skip(1)
        .take_while(|folder| folder.starts_with(root))
        .all(|folder| fs::symlink_metadata(folder).is_ok_and(|meta| meta.file_type().is_dir() && safe(&meta)))
}

/// Whether the install step's script is on this machine under `root`, trusted.
#[must_use]
pub fn script_under(root: &Path) -> Option<PathBuf> {
    let path = root.join(SCRIPT.trim_start_matches('/'));
    trusted(root, &path, ROOT).then_some(path)
}

/// The sudoers line that lets `user` run the script without a password, and nothing else: the
/// empty `""` after it allows the script only without arguments. It is shown for the person to
/// add with visudo; qpac never writes it.
#[must_use]
pub fn sudoers_line(user: &str) -> String {
    format!("{user} ALL=(root) NOPASSWD: {SCRIPT} \"\"")
}

/// What the Settings page and the wizard know about the ladder on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Here {
    /// Whether the install step's script is there and trusted.
    pub script: bool,
    /// The login name the sudoers line is written for; `None` when it cannot be told.
    pub user: Option<String>,
}

impl Here {
    /// Looks under the file system's `root` for the script, which `owner` (root on a real
    /// machine) must own, and in its password database for the name of `uid`.
    #[must_use]
    pub fn detect(root: &Path, owner: u32, uid: Option<u32>) -> Self {
        let user = uid.and_then(|uid| {
            let passwd = fs::read_to_string(root.join("etc/passwd")).ok()?;
            qpackages_core::helper::passwd_name(&passwd, uid)
        });
        let script = trusted(root, &root.join(SCRIPT.trim_start_matches('/')), owner);
        Self { script, user }
    }

    /// The steps that can be chosen here.
    #[must_use]
    pub fn offered(&self) -> Vec<Mode> {
        offered(self.script)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("qpackages-ladder-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("usr/lib/quvyta-packages")).expect("a folder");
        root
    }

    fn mode(path: &Path, bits: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(bits)).expect("permissions");
    }

    #[test]
    fn the_steps_are_read_and_written_by_their_keys() {
        for (step, key) in Mode::ALL {
            assert_eq!(Mode::parse(key), Some(step));
            assert_eq!(step.key(), key);
        }
        assert_eq!(Mode::parse("always"), None);
        assert!(Mode::Notify < Mode::Download && Mode::Download < Mode::Install);
    }

    #[test]
    fn install_is_offered_and_kept_only_where_the_script_is() {
        assert_eq!(offered(false), [Mode::Notify, Mode::Download]);
        assert_eq!(offered(true), [Mode::Notify, Mode::Download, Mode::Install]);
        assert_eq!(Mode::Install.effective(false), Mode::Download);
        assert_eq!(Mode::Install.effective(true), Mode::Install);
        assert_eq!(Mode::Notify.effective(false), Mode::Notify);
    }

    #[test]
    fn only_a_script_nobody_else_can_change_is_trusted() {
        // The test's own user stands in for root, and the scratch folder for `/`: a test cannot
        // make root own a file, and the shared temporary folder above it is anyone's to write.
        let owner = crate::current_uid().expect("the test's own user id");
        let root = scratch("trusted");
        let script = root.join("usr/lib/quvyta-packages/upgrade");
        fs::write(&script, "#!/bin/sh\n").expect("a script");
        for folder in ["", "usr", "usr/lib", "usr/lib/quvyta-packages"] {
            mode(&root.join(folder), 0o755);
        }
        mode(&script, 0o755);
        assert!(trusted(&root, &script, owner));
        assert!(!trusted(&root, &script, owner + 1), "another owner is not trusted");
        mode(&script, 0o775);
        assert!(!trusted(&root, &script, owner), "a group that can write it");
        mode(&script, 0o644);
        assert!(!trusted(&root, &script, owner), "a script that cannot run");
        mode(&script, 0o755);
        for folder in ["", "usr/lib"] {
            mode(&root.join(folder), 0o777);
            assert!(!trusted(&root, &script, owner), "`{folder}` above it that anyone can write");
            mode(&root.join(folder), 0o755);
        }
        let link = root.join("usr/lib/quvyta-packages/link");
        std::os::unix::fs::symlink(&script, &link).expect("a link");
        assert!(!trusted(&root, &link, owner), "a link is never the script");
        assert!(!trusted(&root, &root.join("usr/lib/quvyta-packages/missing"), owner));
        assert!(!trusted(&root.join("usr"), &script.with_file_name("x"), owner));
        assert_eq!(script_under(&root), None, "the test's files are not root's");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_sudoers_line_allows_the_script_without_arguments_only() {
        assert_eq!(sudoers_line("ayse"), "ayse ALL=(root) NOPASSWD: /usr/lib/quvyta-packages/upgrade \"\"");
    }
}

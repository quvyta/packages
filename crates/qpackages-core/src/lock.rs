//! Whether pacman's database is locked, and by whom when that can be told.
//!
//! pacman takes `<dbpath>/db.lck` for the length of a transaction. The lock is never removed
//! from here, not even when it looks stale: a pacman that crashed halfway may have left the
//! database in a state that a second writer would damage. The check is a courtesy before a
//! transaction, not a guarantee: another process may take the lock right after it, and then
//! pacman's own error is shown as it comes.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The state of pacman's database lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockStatus {
    /// No lock file: a transaction can start.
    Free,
    /// The lock file is there: pacman, or something driving it, is in the middle of a transaction.
    Held {
        /// When the lock was taken, read from the file's modification time. `None` when the file
        /// is there but its metadata cannot be read; nothing is guessed in its place.
        since: Option<SystemTime>,
        /// The process holding the lock open, when the scan could see it. `None` means it could
        /// not be told, not that there is none: a root-owned pacman hides its file descriptors
        /// from an ordinary user.
        owner: Option<Owner>,
    },
}

/// A process that holds the lock file open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The process id.
    pub pid: u32,
    /// The program name the kernel reports for the process (`/proc/<pid>/comm`), or `None` when
    /// that file could not be read. The pid is a fact on its own; the name is not invented.
    pub command: Option<String>,
}

/// The lock file pacman keeps under its database directory.
const LOCK_FILE: &str = "db.lck";

/// Where the kernel lists the running processes.
const PROC: &str = "/proc";

/// Looks at the lock under `dbpath`, normally `/var/lib/pacman`.
///
/// When the lock file cannot even be tested for existence, the lock is reported [`LockStatus::Free`]:
/// the check is only a courtesy, and pacman will say for itself when it cannot proceed.
#[must_use]
pub fn lock_status(dbpath: &Path) -> LockStatus {
    let lock = dbpath.join(LOCK_FILE);
    if !lock.try_exists().unwrap_or(false) {
        return LockStatus::Free;
    }
    let since = fs::metadata(&lock).and_then(|metadata| metadata.modified()).ok();
    LockStatus::Held { since, owner: owner_in(Path::new(PROC), &lock) }
}

/// Scans a `/proc`-shaped directory for a process that holds `lock` open.
///
/// Only symlinks are read, so the scan stays quick on a busy machine. A process whose file
/// descriptors cannot be listed is skipped silently, and so is anything that is not a process
/// directory. The first match wins.
fn owner_in(proc: &Path, lock: &Path) -> Option<Owner> {
    let canonical = lock.canonicalize().ok();
    let points_at_lock = |target: &PathBuf| target == lock || canonical.as_ref() == Some(target);
    for entry in proc.read_dir().ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(fds) = entry.path().join("fd").read_dir() else {
            continue;
        };
        let holds_lock = fds.flatten().any(|fd| fs::read_link(fd.path()).is_ok_and(|target| points_at_lock(&target)));
        if holds_lock {
            let command = fs::read_to_string(entry.path().join("comm")).ok().map(|text| text.trim_end().to_owned());
            return Some(Owner { pid, command });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    use super::*;

    /// A fresh directory under the system's temporary place, emptied when it already exists so a
    /// test that was interrupted last time cannot leak into this one.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-lock-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory can be created");
        dir
    }

    #[test]
    fn no_file_means_free() {
        let dir = scratch("free");
        assert_eq!(lock_status(&dir), LockStatus::Free);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_directory_that_cannot_be_read_is_reported_free() {
        let dir = scratch("unreadable-dbpath");
        assert_eq!(lock_status(&dir.join("nowhere")), LockStatus::Free, "the check is a courtesy, not a guarantee");
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_lock_file_is_held_since_its_modification_time() {
        let dir = scratch("held");
        fs::write(dir.join(LOCK_FILE), "").expect("the lock can be written");
        match lock_status(&dir) {
            LockStatus::Held { since: Some(since), owner: None } => {
                let age = SystemTime::now().duration_since(since).expect("the lock is not from the future");
                assert!(age < Duration::from_secs(5), "the lock was taken just now, not {age:?} ago");
            }
            other => panic!("a lock file just written is held with a time and no known owner: {other:?}"),
        }
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn the_owner_is_the_process_whose_descriptor_points_at_the_lock() {
        let dir = scratch("owner");
        let lock = dir.join(LOCK_FILE);
        fs::write(&lock, "").expect("the lock can be written");
        let proc = dir.join("proc");
        fs::create_dir_all(proc.join("4242/fd")).expect("a fake process");
        symlink(&lock, proc.join("4242/fd/3")).expect("a fake descriptor");
        fs::write(proc.join("4242/comm"), "pacman\n").expect("a fake command name");
        fs::create_dir_all(proc.join("7/fd")).expect("a bystander process");
        symlink(dir.join("elsewhere"), proc.join("7/fd/0")).expect("a descriptor on something else");

        assert_eq!(owner_in(&proc, &lock), Some(Owner { pid: 4242, command: Some("pacman".to_owned()) }));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn processes_that_cannot_be_read_are_skipped_and_nothing_is_guessed() {
        let dir = scratch("skipped");
        let lock = dir.join(LOCK_FILE);
        fs::write(&lock, "").expect("the lock can be written");
        let proc = dir.join("proc");
        let hidden = proc.join("1/fd");
        fs::create_dir_all(&hidden).expect("a process that hides its descriptors");
        fs::set_permissions(&hidden, fs::Permissions::from_mode(0o000)).expect("permissions can be taken away");
        fs::create_dir_all(proc.join("2")).expect("a process without an fd directory");
        fs::write(proc.join("3"), "").expect("a name that is a file, not a process");
        fs::create_dir_all(proc.join("sys/fd")).expect("a directory that is not a process");
        symlink(&lock, proc.join("sys/fd/0")).expect("a symlink outside any process");

        assert_eq!(owner_in(&proc, &lock), None, "an owner is only reported when a process is seen holding the lock");
        fs::set_permissions(&hidden, fs::Permissions::from_mode(0o755)).expect("permissions come back for cleanup");
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_process_without_a_comm_file_still_names_its_pid() {
        let dir = scratch("nameless");
        let lock = dir.join(LOCK_FILE);
        fs::write(&lock, "").expect("the lock can be written");
        let proc = dir.join("proc");
        fs::create_dir_all(proc.join("99/fd")).expect("a fake process");
        symlink(&lock, proc.join("99/fd/5")).expect("a fake descriptor");

        assert_eq!(owner_in(&proc, &lock), Some(Owner { pid: 99, command: None }));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_proc_directory_that_is_missing_finds_nobody() {
        let dir = scratch("no-proc");
        assert_eq!(owner_in(&dir.join("proc"), &dir.join(LOCK_FILE)), None);
        fs::remove_dir_all(dir).expect("cleanup");
    }
}

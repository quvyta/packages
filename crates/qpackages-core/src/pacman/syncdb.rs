//! A private copy of the sync database, so updates can be checked without touching the system.
//!
//! Refreshing the system database (`pacman -Sy`) without upgrading right after is how partial
//! upgrades happen: the next `pacman -S something` pulls in libraries newer than what the rest of
//! the system was built against. The safe way, the one `checkupdates` takes, is to copy the sync
//! database somewhere private, refresh only that copy, and ask `pacman -Qu` about it. The system
//! database is only ever read here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The subdirectory pacman expects the repository databases in under a `--dbpath`.
const SYNC_DIR: &str = "sync";

/// The subdirectory pacman expects the records of installed packages in under a `--dbpath`.
const LOCAL_DIR: &str = "local";

/// The extension of a repository database. The `.files` databases beside them are not copied:
/// they hold file lists, which an update check never reads, and they are many times larger.
const DB_EXTENSION: &str = "db";

/// Copies the repository databases from `system_sync`, normally `/var/lib/pacman/sync`, into
/// `<work>/sync`, links `<work>/local` to the system's installed-package records beside
/// `system_sync`, and returns `work`: the `--dbpath` to give pacman. Without that link the copy
/// knows of nothing installed, and so of no update.
///
/// A database is copied only when the copy is missing or older than the system's, so a check
/// that runs every few minutes does not move megabytes each time. Nothing under `system_sync`
/// is written. A missing or unreadable `system_sync` is an error, not a panic.
pub fn prepare(system_sync: &Path, work: &Path) -> io::Result<PathBuf> {
    let target = work.join(SYNC_DIR);
    fs::create_dir_all(&target)?;
    link_local(&system_sync.parent().unwrap_or(system_sync).join(LOCAL_DIR), work)?;
    for entry in fs::read_dir(system_sync)? {
        let entry = entry?;
        let source = entry.path();
        if source.extension().is_none_or(|extension| extension != DB_EXTENSION) || !entry.file_type()?.is_file() {
            continue;
        }
        let copy = target.join(entry.file_name());
        if is_current(&copy, &entry.metadata()?)? {
            continue;
        }
        fs::copy(&source, &copy)?;
    }
    Ok(work.to_path_buf())
}

/// Makes `<work>/local` point at the system's record of installed packages, `local`, normally
/// `/var/lib/pacman/local`, so `pacman -Qu --dbpath <work>` compares what is installed with the
/// private copy of the repositories, the way `checkupdates` does. A link is enough: the check only
/// reads the records. A link already pointing there is left alone; anything else in its place is
/// an error rather than something removed.
pub fn link_local(local: &Path, work: &Path) -> io::Result<()> {
    fs::create_dir_all(work)?;
    let link = work.join(LOCAL_DIR);
    match fs::read_link(&link) {
        Ok(target) if target == local => Ok(()),
        Ok(_) => {
            fs::remove_file(&link)?;
            std::os::unix::fs::symlink(local, &link)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::os::unix::fs::symlink(local, &link),
        Err(error) => Err(error),
    }
}

/// Whether `copy` exists and is at least as new as the source it was taken from.
///
/// The copy gets the time it was written, which is after the source's, so a copy taken once
/// stays current until the system database is refreshed again.
fn is_current(copy: &Path, source: &fs::Metadata) -> io::Result<bool> {
    match fs::metadata(copy) {
        Ok(existing) => Ok(existing.modified()? >= source.modified()?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    /// A fresh directory under the system's temporary place, emptied when it already exists so a
    /// test that was interrupted last time cannot leak into this one.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-syncdb-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory can be created");
        dir
    }

    /// A system sync directory with three repository databases and one file database.
    fn fake_system(dir: &Path) -> PathBuf {
        let system = dir.join("system");
        fs::create_dir_all(&system).expect("the fake system directory");
        for name in ["core.db", "extra.db", "multilib.db", "core.files"] {
            fs::write(system.join(name), format!("contents of {name}")).expect("a fake database");
        }
        system
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("the directory can be listed")
            .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn copies_the_databases_and_leaves_the_file_lists_behind() {
        let dir = scratch("copies");
        let system = fake_system(&dir);
        let work = dir.join("work");

        let dbpath = prepare(&system, &work).expect("the copy succeeds");

        assert_eq!(dbpath, work, "the returned path is the one to give pacman as --dbpath");
        assert_eq!(names_in(&work.join("sync")), ["core.db", "extra.db", "multilib.db"]);
        assert_eq!(fs::read_to_string(work.join("sync/extra.db")).expect("the copy reads"), "contents of extra.db");
        assert_eq!(names_in(&system), ["core.db", "core.files", "extra.db", "multilib.db"], "the source is untouched");
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn the_installed_records_are_linked_not_copied_and_a_second_link_changes_nothing() {
        let dir = scratch("link");
        let local = dir.join("system-local");
        fs::create_dir_all(&local).expect("the fake records");
        let work = dir.join("work");
        link_local(&local, &work).expect("the link is made");
        assert_eq!(fs::read_link(work.join("local")).expect("a link"), local);
        link_local(&local, &work).expect("an existing link is fine");
        let elsewhere = dir.join("elsewhere");
        link_local(&elsewhere, &work).expect("a stale link is replaced");
        assert_eq!(fs::read_link(work.join("local")).expect("a link"), elsewhere);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_real_folder_where_the_link_goes_is_an_error_and_is_kept() {
        let dir = scratch("link-folder");
        let work = dir.join("work");
        fs::create_dir_all(work.join("local")).expect("a folder in the way");
        assert!(link_local(&dir.join("system-local"), &work).is_err());
        assert!(work.join("local").is_dir(), "nothing is removed");
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn the_copy_sees_the_installed_packages_through_a_link() {
        // Without the local database beside the copy, `pacman -Qu --dbpath <copy>` finds nothing
        // installed and so never any update.
        let dir = scratch("local");
        let system = fake_system(&dir);
        fs::create_dir_all(dir.join("local")).expect("the local database");
        let work = dir.join("work");
        prepare(&system, &work).expect("the copy succeeds");
        assert_eq!(fs::read_link(work.join("local")).expect("a link"), dir.join("local"));
        prepare(&system, &work).expect("preparing again keeps the link");
        assert_eq!(fs::read_link(work.join("local")).expect("a link"), dir.join("local"));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_missing_source_is_an_error_not_a_panic() {
        let dir = scratch("missing");
        let result = prepare(&dir.join("nowhere"), &dir.join("work"));
        assert!(result.is_err());
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn preparing_twice_keeps_a_copy_that_is_still_current() {
        let dir = scratch("twice");
        let system = fake_system(&dir);
        let work = dir.join("work");
        prepare(&system, &work).expect("the first copy succeeds");

        // The copy was written after the source, so it counts as current and is left alone.
        let copy = work.join("sync/core.db");
        fs::write(&copy, "refreshed privately").expect("the copy can be marked");
        prepare(&system, &work).expect("the second copy succeeds");
        assert_eq!(fs::read_to_string(&copy).expect("the copy reads"), "refreshed privately");

        // Once the system database is newer than the copy, the copy is taken again.
        let later = SystemTime::now() + Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(system.join("core.db"))
            .and_then(|file| file.set_modified(later))
            .expect("the source can be dated");
        prepare(&system, &work).expect("the third copy succeeds");
        assert_eq!(fs::read_to_string(&copy).expect("the copy reads"), "contents of core.db");
        fs::remove_dir_all(dir).expect("cleanup");
    }
}

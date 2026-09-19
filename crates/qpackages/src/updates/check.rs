//! Looking for updates without touching the system: the repositories through a private copy of
//! pacman's database, the AUR through its helper.
//!
//! Refreshing the system's own database without upgrading right after is how partial upgrades
//! happen, so the check copies the repository databases into a folder of qpac's, refreshes only
//! that copy (with fakeroot, since pacman insists on root to sync) and asks `pacman -Qu` against
//! it, the way `checkupdates` does. Nothing here needs or asks for privileges.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use qpackages_core::pacman::command::{self, FAKEROOT, PACMAN};
use qpackages_core::pacman::syncdb;
use qpackages_core::pacman::{Update, UpdateCheck, read_update_check};

use crate::reload::Lookup;
use crate::runner::Runner;

/// Why a part of the check did not run to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// There is no folder to keep the private copy in: the account has no home folder.
    NoPlace,
    /// fakeroot is not installed, and pacman refuses to refresh even a copy without it.
    NoFakeroot,
    /// A program ended without success, or could not be started; what it said, in English, for
    /// the user to read under the explanation.
    Said(String),
}

/// What one check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// When the check ended, as a Unix timestamp.
    pub at: i64,
    /// The repositories' updates, or why they are unknown.
    pub repo: Result<Vec<Update>, Failure>,
    /// The AUR's updates, or why they are unknown; `None` when the AUR was not asked, because it
    /// is turned off, has no helper, or qpac runs as root.
    pub aur: Option<Result<Vec<Update>, Failure>>,
}

/// Everything a check needs, kept so it can run again whenever the user asks.
#[derive(Clone)]
pub struct Checker {
    runner: Arc<dyn Runner>,
    lookup: Arc<Lookup>,
    /// pacman's records of the installed packages, normally `/var/lib/pacman/local`.
    local: PathBuf,
    /// pacman's repository databases, normally `/var/lib/pacman/sync`; only ever read.
    sync: PathBuf,
    /// The folder of the private copy, when the account has somewhere to keep it.
    work: Option<PathBuf>,
}

impl std::fmt::Debug for Checker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checker")
            .field("local", &self.local)
            .field("sync", &self.sync)
            .field("work", &self.work)
            .finish_non_exhaustive()
    }
}

impl Checker {
    /// A check that copies `sync` into `work`, links `local` beside it, and runs its programs
    /// through `runner`, found through `lookup`.
    #[must_use]
    pub fn new(runner: Arc<dyn Runner>, lookup: Arc<Lookup>, local: &Path, sync: &Path, work: Option<&Path>) -> Self {
        Self { runner, lookup, local: local.to_path_buf(), sync: sync.to_path_buf(), work: work.map(Path::to_path_buf) }
    }

    /// Checks here and now; `aur_helper` is the program asked about the AUR, if it is to be
    /// asked. Meant for a background thread: the refresh downloads the repository databases.
    #[must_use]
    pub fn run(&self, aur_helper: Option<&str>) -> Found {
        let repo = self.repositories();
        let aur = aur_helper.map(|helper| self.query(helper, &command::aur_update_check()));
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        Found { at: i64::try_from(at).unwrap_or(i64::MAX), repo, aur }
    }

    fn repositories(&self) -> Result<Vec<Update>, Failure> {
        let work = self.work.as_deref().ok_or(Failure::NoPlace)?;
        if (self.lookup)(FAKEROOT).is_none() {
            return Err(Failure::NoFakeroot);
        }
        let dbpath = syncdb::prepare(&self.sync, work).map_err(|error| Failure::Said(error.to_string()))?;
        syncdb::link_local(&self.local, &dbpath).map_err(|error| Failure::Said(error.to_string()))?;
        let refresh = self
            .runner
            .output(FAKEROOT, &command::refresh(&dbpath), &command::parsed_env())
            .map_err(|error| Failure::Said(error.to_string()))?;
        if !refresh.succeeded() {
            return Err(Failure::Said(first_line(&refresh.stderr, &refresh.stdout)));
        }
        self.query(PACMAN, &command::update_check(&dbpath))
    }

    /// Runs one `-Qu`-shaped query and reads what it meant.
    fn query(&self, program: &str, args: &[String]) -> Result<Vec<Update>, Failure> {
        let output = self
            .runner
            .output(program, args, &command::parsed_env())
            .map_err(|error| Failure::Said(error.to_string()))?;
        match read_update_check(output.code, &output.stdout, &output.stderr) {
            UpdateCheck::Updates(updates) => Ok(updates),
            UpdateCheck::Failed { stderr } => Err(Failure::Said(first_line(&stderr, &output.stdout))),
        }
    }
}

/// The first line worth showing of what a program said: its error stream first, since that is
/// where pacman explains itself.
fn first_line(stderr: &str, stdout: &str) -> String {
    stderr.lines().chain(stdout.lines()).map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::runner::Recorded;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-check-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sync")).expect("a fake sync folder");
        fs::write(dir.join("sync/core.db"), "core").expect("a fake database");
        fs::create_dir_all(dir.join("local")).expect("a fake local folder");
        dir
    }

    fn with_fakeroot(program: &str) -> Option<PathBuf> {
        (program == FAKEROOT).then(|| PathBuf::from("/usr/bin/fakeroot"))
    }

    fn checker(dir: &Path, recorded: &Arc<Recorded>, lookup: Arc<Lookup>) -> Checker {
        let runner = Arc::clone(recorded) as Arc<dyn Runner>;
        Checker::new(runner, lookup, &dir.join("local"), &dir.join("sync"), Some(&dir.join("work")))
    }

    #[test]
    fn the_copy_is_refreshed_then_asked_and_the_aur_helper_after_it() {
        let dir = scratch("both");
        let work = dir.join("work");
        let recorded = Arc::new(Recorded::default());
        recorded.answer(FAKEROOT, &command::refresh(&work), "", 0);
        recorded.answer(PACMAN, &command::update_check(&work), "linux 6.18.1-1 -> 6.18.2-1\n", 0);
        recorded.answer("paru", &command::aur_update_check(), "brave-bin 1:1.95.101-1 -> 1:1.95.102-1\n", 0);

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(Some("paru"));

        assert_eq!(found.repo.expect("the repositories answered").len(), 1);
        assert_eq!(found.aur.expect("the AUR was asked").expect("and answered")[0].name, "brave-bin");
        assert!(found.at > 0);
        assert_eq!(
            recorded.command_lines(),
            [
                format!("fakeroot -- pacman -Sy --dbpath {} --logfile /dev/null --disable-sandbox", work.display()),
                format!("pacman -Qu --dbpath {}", work.display()),
                "paru -Qua".to_owned(),
            ]
        );
        assert!(recorded.calls().iter().all(|call| call.env.contains(&("LC_ALL".to_owned(), "C".to_owned()))));
        assert_eq!(fs::read_link(work.join("local")).expect("the records are linked"), dir.join("local"));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn nothing_runs_without_fakeroot_or_a_place_for_the_copy() {
        let dir = scratch("missing");
        let recorded = Arc::new(Recorded::default());
        let found = checker(&dir, &recorded, Arc::new(|_| None)).run(None);
        assert_eq!(found.repo, Err(Failure::NoFakeroot));
        assert_eq!(found.aur, None, "the AUR is not asked when no helper is given");
        let placeless =
            Checker::new(Arc::clone(&recorded) as Arc<dyn Runner>, Arc::new(with_fakeroot), &dir, &dir, None);
        assert_eq!(placeless.run(None).repo, Err(Failure::NoPlace));
        assert!(recorded.calls().is_empty());
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_failed_refresh_says_what_pacman_said_and_asks_nothing_more() {
        let dir = scratch("offline");
        let work = dir.join("work");
        let recorded = Arc::new(Recorded::default());
        let stderr = "\nerror: failed retrieving file 'core.db' from mirror : Could not resolve host\n";
        recorded.fail(FAKEROOT, &command::refresh(&work), stderr, 1);
        recorded.answer("paru", &command::aur_update_check(), "", 1);

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(Some("paru"));

        assert_eq!(
            found.repo,
            Err(Failure::Said(
                "error: failed retrieving file 'core.db' from mirror : Could not resolve host".to_owned()
            ))
        );
        assert_eq!(found.aur, Some(Ok(Vec::new())), "the AUR is asked all the same, and is up to date");
        fs::remove_dir_all(dir).expect("cleanup");
    }
}

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

use qframe::prelude::t;
use qpackages_core::flatpak::{self, Scope};
use qpackages_core::pacman::command::{self, FAKEROOT, PACMAN};
use qpackages_core::pacman::syncdb;
use qpackages_core::pacman::{Update, UpdateCheck, read_update_check};
use qpackages_core::snap;

use crate::reload::Lookup;
use crate::runner::Runner;
use crate::snap::Snapd;

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
    /// One installation answered and another did not. The updates that did arrive stay beside the
    /// quiet reason instead of disappearing with the failed scope.
    Partial {
        /// The updates from every installation that answered.
        updates: Vec<Update>,
        /// Why the other installation could not be read.
        failure: Box<Failure>,
    },
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
    /// The Flatpak refs with a newer commit waiting, or why they are unknown; `None` when Flatpak
    /// was not asked, because it is turned off or is not installed.
    pub flatpak: Option<Result<Vec<Update>, Failure>>,
    /// The snaps with a newer version waiting, or why they are unknown; `None` when Snap was not
    /// asked, because it is turned off or snapd is not answering.
    pub snap: Option<Result<Vec<Update>, Failure>>,
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

    /// Checks here and now. The AUR helper, Flatpak and snapd are asked only when each is to be
    /// asked. Meant for a background thread: the refresh downloads the repository databases.
    #[must_use]
    pub fn run(&self, aur_helper: Option<&str>, snapd: Option<&Snapd>, flatpak: Option<&str>) -> Found {
        let repo = self.repositories();
        let aur = aur_helper.map(|helper| self.query(helper, &command::aur_update_check()));
        let flatpak = flatpak.map(|program| self.flatpak(program));
        let snap = snapd.map(|snapd| self.snaps(snapd));
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        Found { at: i64::try_from(at).unwrap_or(i64::MAX), repo, aur, flatpak, snap }
    }

    /// The Flatpak refs with a newer commit waiting across the user's and the machine's
    /// installations. Each scope is independent, so one broken installation never hides rows the
    /// other one found.
    fn flatpak(&self, program: &str) -> Result<Vec<Update>, Failure> {
        let mut updates = Vec::new();
        let mut answered = false;
        let mut failure = None;
        for scope in [Scope::User, Scope::System] {
            match self.flatpak_scope(program, scope) {
                Ok(found) => {
                    answered = true;
                    updates.extend(found);
                }
                Err(reason) => failure = failure.or(Some(reason)),
            }
        }
        match (answered, failure) {
            (false, Some(failure)) => Err(failure),
            (true, Some(failure)) => Err(Failure::Partial { updates, failure: Box::new(failure) }),
            (true, None) => Ok(updates),
            (false, None) => Err(Failure::Said(String::new())),
        }
    }

    /// One Flatpak installation's updates, paired with the versions that installation has now.
    fn flatpak_scope(&self, program: &str, scope: Scope) -> Result<Vec<Update>, Failure> {
        let listed = self
            .runner
            .output(program, &flatpak::updates_args(scope), &command::parsed_env())
            .map_err(|error| Failure::Said(error.to_string()))?;
        let installed = self
            .runner
            .output(program, &flatpak::installed_args(scope), &command::parsed_env())
            .map_err(|error| Failure::Said(error.to_string()))?;
        match flatpak::read_updates(
            listed.code,
            &listed.stdout,
            &listed.stderr,
            installed.code,
            &installed.stdout,
            &installed.stderr,
        ) {
            flatpak::UpdateCheck::Updates(updates) => Ok(updates),
            flatpak::UpdateCheck::Failed { stderr } => Err(Failure::Said(stderr)),
            flatpak::UpdateCheck::Unreadable => Err(Failure::Said(t!("updates.flatpak-unreadable"))),
        }
    }

    /// The snaps with a newer version waiting.
    ///
    /// `snap refresh --list` names each snap and the version that would be installed; the version
    /// it has now comes from snapd's own list of what is installed, which is read on the socket.
    /// Both run as the user: snapd asks for no permission to be asked about anything.
    ///
    /// Nothing to update prints one line on the error stream and ends with 0, so an empty answer
    /// is the usual case, not a failure.
    fn snaps(&self, snapd: &Snapd) -> Result<Vec<Update>, Failure> {
        let installed = snapd.installed().map_err(|_| Failure::Said(t!("snap.unreachable")))?;
        let listed = self
            .runner
            .output(snap::SNAP, &snap::refresh_list_args(), &command::parsed_env())
            .map_err(|error| Failure::Said(error.to_string()))?;
        if !listed.succeeded() && snap::trouble(&listed.stderr) != Some(snap::Trouble::NoUpdates) {
            return Err(Failure::Said(first_line(&listed.stderr, &listed.stdout)));
        }
        Ok(snap::parse_refresh_list(&listed.stdout)
            .into_iter()
            .map(|waiting| {
                let from = installed
                    .iter()
                    .find(|snap| snap.name == waiting.name)
                    .map(|snap| snap.version.clone())
                    .unwrap_or_default();
                Update { name: waiting.name, from, to: waiting.version, ignored: false }
            })
            .collect())
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
    use qpackages_core::catalog::flatpak::FLATPAK;

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

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(Some("paru"), None, None);

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
        let found = checker(&dir, &recorded, Arc::new(|_| None)).run(None, None, None);
        assert_eq!(found.repo, Err(Failure::NoFakeroot));
        assert_eq!(found.aur, None, "the AUR is not asked when no helper is given");
        let placeless =
            Checker::new(Arc::clone(&recorded) as Arc<dyn Runner>, Arc::new(with_fakeroot), &dir, &dir, None);
        assert_eq!(placeless.run(None, None, None).repo, Err(Failure::NoPlace));
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

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(Some("paru"), None, None);

        assert_eq!(
            found.repo,
            Err(Failure::Said(
                "error: failed retrieving file 'core.db' from mirror : Could not resolve host".to_owned()
            ))
        );
        assert_eq!(found.aur, Some(Ok(Vec::new())), "the AUR is asked all the same, and is up to date");
        fs::remove_dir_all(dir).expect("cleanup");
    }

    /// The snap recordings the container produced.
    fn snap_fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/snap").join(name);
        fs::read_to_string(path).expect("the recording is readable")
    }

    /// The Flatpak recordings the container produced.
    fn flatpak_fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/flatpak").join(name);
        fs::read_to_string(path).expect("the recording is readable")
    }

    #[test]
    fn both_flatpak_installations_are_asked_for_updates() {
        // Recorded in the throwaway Arch container with Flatpak 1.18.3.
        let dir = scratch("flatpak-both");
        let recorded = Arc::new(Recorded::default());
        recorded.answer(FLATPAK, &flatpak::updates_args(Scope::User), &flatpak_fixture("updates-user.out"), 0);
        recorded.answer(FLATPAK, &flatpak::installed_args(Scope::User), &flatpak_fixture("list-user.out"), 0);
        recorded.answer(FLATPAK, &flatpak::updates_args(Scope::System), &flatpak_fixture("updates-user-empty.out"), 0);
        recorded.answer(FLATPAK, &flatpak::installed_args(Scope::System), &flatpak_fixture("list-system.out"), 0);
        let checker = Checker::new(Arc::clone(&recorded) as Arc<dyn Runner>, Arc::new(with_fakeroot), &dir, &dir, None);

        let found = checker.run(None, None, Some(FLATPAK));

        let updates = found.flatpak.expect("Flatpak was asked").expect("both installations answered");
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].name, "net.sourceforge.ExtremeTuxRacer");
        assert_eq!(updates[1].name, "org.freedesktop.Platform");
        assert_eq!(
            recorded.command_lines(),
            [
                "flatpak --user remote-ls --updates --columns=application,branch,version,commit",
                "flatpak --user list --columns=application,version,branch,installation",
                "flatpak --system remote-ls --updates --columns=application,branch,version,commit",
                "flatpak --system list --columns=application,version,branch,installation",
            ]
        );
        assert!(recorded.calls().iter().all(|call| call.env.contains(&("LC_ALL".to_owned(), "C".to_owned()))));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn one_broken_flatpak_installation_does_not_hide_the_other_updates() {
        // Recorded in the throwaway Arch container with Flatpak 1.18.3.
        let dir = scratch("flatpak-partial");
        let recorded = Arc::new(Recorded::default());
        recorded.answer_full(
            FLATPAK,
            &flatpak::updates_args(Scope::User),
            "",
            &flatpak_fixture("updates-error.err"),
            1,
        );
        recorded.answer(FLATPAK, &flatpak::installed_args(Scope::User), &flatpak_fixture("list-user.out"), 0);
        recorded.answer(FLATPAK, &flatpak::updates_args(Scope::System), &flatpak_fixture("updates-user.out"), 0);
        recorded.answer(FLATPAK, &flatpak::installed_args(Scope::System), &flatpak_fixture("list-user.out"), 0);
        let checker = Checker::new(Arc::clone(&recorded) as Arc<dyn Runner>, Arc::new(with_fakeroot), &dir, &dir, None);

        let found = checker.run(None, None, Some(FLATPAK));

        let Err(Failure::Partial { updates, failure }) = found.flatpak.expect("Flatpak was asked") else {
            panic!("one installation failed")
        };
        assert_eq!(updates.len(), 2, "the system installation's rows still arrive");
        assert_eq!(
            failure.as_ref(),
            &Failure::Said("error: Remote \"no-such-remote\" not found in the user installation".to_owned())
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn the_snaps_with_an_update_pair_the_new_version_with_the_one_installed() {
        let dir = scratch("snaps");
        let socket = dir.join("snapd.socket");
        let fake = crate::testing::FakeSnapd::start(&socket);
        fake.answer(qpackages_core::snap::api::SNAPS, &snap_fixture("api-snaps.json"));
        let recorded = Arc::new(Recorded::default());
        recorded.answer(snap::SNAP, &snap::refresh_list_args(), &snap_fixture("refresh-list.out"), 0);
        let snapd = Snapd::new(true, &socket);

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(None, Some(&snapd), None);

        let snaps = found.snap.expect("snapd was asked").expect("and answered");
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].name, "hello");
        assert_eq!(snaps[0].to, "2.10", "the version waiting comes from the list");
        assert_eq!(snaps[0].from, "2.10", "the one installed comes from snapd's own list");
        assert!(!snaps[0].ignored, "snapd holds nothing back");
        assert!(recorded.command_lines().contains(&format!("{} refresh --list", snap::SNAP)));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn nothing_to_update_is_an_empty_answer_rather_than_a_failure() {
        let dir = scratch("snaps-none");
        let socket = dir.join("snapd.socket");
        let fake = crate::testing::FakeSnapd::start(&socket);
        fake.answer(qpackages_core::snap::api::SNAPS, "{\"result\":[]}");
        let recorded = Arc::new(Recorded::default());
        // `All snaps up to date.` goes to the error stream, and snap still ends with 0.
        recorded.answer_full(snap::SNAP, &snap::refresh_list_args(), "", &snap_fixture("refresh-list-none.err"), 0);
        let snapd = Snapd::new(true, &socket);

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(None, Some(&snapd), None);

        assert_eq!(found.snap.expect("snapd was asked").expect("and answered"), Vec::<Update>::new());
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_snapd_that_cannot_be_reached_is_a_failure_and_never_runs_the_snap_program() {
        let dir = scratch("snaps-mute");
        let recorded = Arc::new(Recorded::default());
        // Nothing listens on this path, so reading what is installed fails at once.
        let snapd = Snapd::new(true, dir.join("snapd.socket"));

        let found = checker(&dir, &recorded, Arc::new(with_fakeroot)).run(None, Some(&snapd), None);

        assert!(found.snap.expect("snapd was asked").is_err(), "the list is left as it was");
        assert!(
            !recorded.command_lines().iter().any(|line| line.starts_with(snap::SNAP)),
            "the program is never run without a snapd to talk to"
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }
}

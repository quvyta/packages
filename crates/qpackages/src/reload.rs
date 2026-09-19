//! The read that fills the screen: the installed packages, which of them are applications, which
//! no repository offers and which are orphans, and which sources this machine has.
//!
//! It runs when the application starts and again after a transaction changed something, in the
//! background both times, so the screen never waits for it. A local database of some thousand
//! records takes milliseconds; finding the applications reads every package's file list, which
//! takes a fraction of a second once the system has the files in memory and longer on a cold
//! start, and the first frame is drawn before any of it.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::prelude::Command;
use qpackages_core::pacman::command::{self, PACMAN};
use qpackages_core::pacman::{Package, Problem, read_local_db};
use qpackages_core::sources::{AurPreference, Sources, detect};

use crate::app::Msg as AppMsg;
use crate::installed::apps;
use crate::installed::table::Foreign;
use crate::runner::Runner;
use crate::transaction::list_orphans;

/// Finds a program on this machine, as `on_path` does. Shared with the background read, so it
/// must be safe to call from another thread.
pub type Lookup = dyn Fn(&str) -> Option<PathBuf> + Send + Sync;

/// What one read found.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Every installed package, in name order.
    pub packages: Vec<Package>,
    /// Records the database read could not use.
    pub problems: Vec<Problem>,
    /// Which sources this machine has.
    pub sources: Sources,
    /// The names of the packages that put a launcher in the application menu.
    pub apps: BTreeSet<String>,
    /// The names of the packages no repository offers, when pacman could say.
    pub foreign: Foreign,
    /// The orphans: installed as dependencies and needed by nothing now, when pacman could say.
    pub orphans: Foreign,
}

/// Everything a read needs, kept so the read can be repeated after a transaction.
#[derive(Clone)]
pub struct Reload {
    /// Where pacman keeps the records of installed packages.
    dbpath: PathBuf,
    /// Which AUR helper to prefer when several are installed.
    preference: AurPreference,
    /// Where the application menu's launchers are.
    applications: PathBuf,
    lookup: Arc<Lookup>,
    /// Asks pacman which packages no repository offers.
    runner: Arc<dyn Runner>,
}

impl fmt::Debug for Reload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reload")
            .field("dbpath", &self.dbpath)
            .field("preference", &self.preference)
            .field("applications", &self.applications)
            .finish_non_exhaustive()
    }
}

impl Reload {
    /// A read of the packages under `dbpath`, of the launchers under `applications` and of the
    /// sources' programs through `lookup`, choosing the AUR helper by `preference` and asking
    /// pacman through `runner` which packages no repository offers.
    #[must_use]
    pub fn new(
        dbpath: &Path,
        applications: &Path,
        preference: AurPreference,
        lookup: Arc<Lookup>,
        runner: Arc<dyn Runner>,
    ) -> Self {
        Self { dbpath: dbpath.to_path_buf(), preference, applications: applications.to_path_buf(), lookup, runner }
    }

    /// Chooses the AUR helper by `preference` from the next read on.
    pub fn set_preference(&mut self, preference: AurPreference) {
        self.preference = preference;
    }

    /// Reads the packages and looks for the sources' programs, here and now.
    #[must_use]
    pub fn read(&self) -> Snapshot {
        let (packages, problems) = read_local_db(&self.dbpath);
        let sources = detect(self.preference, self.lookup.as_ref());
        let apps = apps::owners(&self.dbpath, &packages, &apps::launchers(&self.applications));
        let foreign = self.foreign();
        let orphans = list_orphans(self.runner.as_ref()).ok().map(|names| names.into_iter().collect());
        Snapshot { packages, problems, sources, apps, foreign, orphans }
    }

    /// The packages no repository offers, from `pacman -Qqm`; `None` when pacman could not say,
    /// so no package is wrongly called one of the repositories'. pacman exits 1 when there are
    /// none, with nothing on either stream.
    fn foreign(&self) -> Foreign {
        let output = self.runner.output(PACMAN, &command::foreign(), &command::parsed_env()).ok()?;
        let nothing = output.code == Some(1) && output.stdout.is_empty() && output.stderr.is_empty();
        (output.succeeded() || nothing).then(|| output.stdout.lines().map(str::to_owned).collect())
    }

    /// Reads in the background and delivers what was found as [`AppMsg::Reloaded`].
    #[must_use]
    pub fn command(&self) -> Command<AppMsg> {
        let reload = self.clone();
        Command::perform(move || AppMsg::Reloaded(reload.read()))
    }
}

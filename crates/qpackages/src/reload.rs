//! The read that fills the screen: the installed packages and which sources this machine has.
//!
//! It runs when the application starts and again after a transaction changed something, in the
//! background both times, so the screen never waits for it: a local database of some thousand
//! records takes milliseconds, but the first frame is drawn before even those pass.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::prelude::Command;
use qpackages_core::pacman::{Package, Problem, read_local_db};
use qpackages_core::sources::{AurPreference, Sources, detect};

use crate::app::Msg as AppMsg;

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
}

/// Everything a read needs, kept so the read can be repeated after a transaction.
#[derive(Clone)]
pub struct Reload {
    /// Where pacman keeps the records of installed packages.
    dbpath: PathBuf,
    /// Which AUR helper to prefer when several are installed.
    preference: AurPreference,
    lookup: Arc<Lookup>,
}

impl fmt::Debug for Reload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reload")
            .field("dbpath", &self.dbpath)
            .field("preference", &self.preference)
            .finish_non_exhaustive()
    }
}

impl Reload {
    /// A read of the packages under `dbpath` and of the sources' programs through `lookup`,
    /// choosing the AUR helper by `preference`.
    #[must_use]
    pub fn new(dbpath: &Path, preference: AurPreference, lookup: Arc<Lookup>) -> Self {
        Self { dbpath: dbpath.to_path_buf(), preference, lookup }
    }

    /// Reads the packages and looks for the sources' programs, here and now.
    #[must_use]
    pub fn read(&self) -> Snapshot {
        let (packages, problems) = read_local_db(&self.dbpath);
        let sources = detect(self.preference, self.lookup.as_ref());
        Snapshot { packages, problems, sources }
    }

    /// Reads in the background and delivers what was found as [`AppMsg::Reloaded`].
    #[must_use]
    pub fn command(&self) -> Command<AppMsg> {
        let reload = self.clone();
        Command::perform(move || AppMsg::Reloaded(reload.read()))
    }
}

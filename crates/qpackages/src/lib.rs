//! qpackages: pacman, AUR, Flatpak and Snap packages in one simple place.
//!
//! The program is [`run`]. The rest of what is public lets a test build the screen over a
//! pretend machine: [`app::Qpackages`] on an [`app::Machine`] whose programs answer through a
//! [`Runner`] and whose administrator helper is started by a [`Start`], drawn with the
//! language files and key bindings of [`asset_dirs`]. Nothing here reaches pacman or sudo
//! unless the machine given says so.

pub mod app;
mod detail;
mod helper;
mod packages;
mod reload;
mod runner;
mod settings;
mod sources;
mod transaction;

pub use helper::session::{Connection, Host, Start};
pub use reload::{Lookup, Snapshot};
pub use runner::{Output, Runner};
pub use transaction::Msg as TransactionMsg;

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;

use qframe::runtime::Runtime;
use qpackages_core::sources::on_path;

/// The language files, compiled in so an installed binary needs nothing beside it.
const LOCALES: [(&str, &str); 2] =
    [("en.toml", include_str!("../assets/locales/en.toml")), ("tr.toml", include_str!("../assets/locales/tr.toml"))];

/// The application's own key bindings, compiled in for the same reason.
const KEYMAP: (&str, &str) = ("keymap.toml", include_str!("../assets/keymap.toml"));

/// Where pacman keeps the records of installed packages.
const LOCAL_DB: &str = "/var/lib/pacman/local";

/// Where pacman keeps its lock file while a transaction runs.
const PACMAN_DIR: &str = "/var/lib/pacman";

/// The user id this process runs as, read from the owner of its own `/proc` entry: root when
/// started with `sudo`, which the screen warns about. `None` when `/proc` cannot be read.
fn current_uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|metadata| metadata.uid())
}

/// Starts the application on the terminal: loads the settings, the compiled-in language files
/// and key bindings, and runs the screen until the user quits.
///
/// Started with `--privileged-helper` as its first argument, the program is the root helper
/// instead: it draws nothing and answers requests on its standard input until that ends.
///
/// qpackages has no settings page yet, so whatever the settings reported (a file left in the
/// old folder, a value that was repaired) is written to standard error once the screen is gone,
/// where the user reads it after quitting.
///
/// # Errors
///
/// Returns the terminal's error when the screen cannot be set up or drawn, or the helper's when
/// its input or output fails.
pub fn run() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args_os().skip(1).map(|arg| arg.to_string_lossy().into_owned()).collect();
    if let Some((first, rest)) = args.split_first()
        && first == qpackages_core::helper::FLAG
    {
        return helper::root::run(rest);
    }
    let settings = settings::load();
    let machine = app::Machine {
        dbpath: Path::new(LOCAL_DB),
        lock_dir: Path::new(PACMAN_DIR),
        lookup: Arc::new(on_path),
        runner: Arc::new(runner::Real),
        helper: Arc::new(helper::session::sudo),
        uid: current_uid(),
    };
    let app = app::Qpackages::new(machine, &settings);
    let result = LOCALES
        .iter()
        .fold(Runtime::new(app), |runtime, (file, text)| runtime.locale_source(*file, *text))
        .keymap_source(KEYMAP.0, KEYMAP.1)
        .settings(&settings)
        .run();
    for diagnostic in settings.diagnostics() {
        eprintln!("{diagnostic}");
    }
    result
}

/// The compiled-in language files and key bindings, as the runtime loads them, for drawing the
/// screen outside a terminal: `qframe::env::Env::load` turns them into the environment a test
/// harness takes.
#[must_use]
pub fn asset_dirs() -> qframe::env::AssetDirs {
    qframe::env::AssetDirs {
        locale_sources: LOCALES.iter().map(|(file, text)| ((*file).to_owned(), (*text).to_owned())).collect(),
        keymap_source: Some((KEYMAP.0.to_owned(), KEYMAP.1.to_owned())),
        ..qframe::env::AssetDirs::default()
    }
}

/// The built-in files plus the compiled-in locales and keymap, as the runtime loads them, for
/// tests that drive the screen.
#[cfg(test)]
fn test_env() -> qframe::env::Env {
    qframe::env::Env::load(&asset_dirs()).expect("the locales and the keymap are readable")
}

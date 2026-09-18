//! qpackages: pacman, AUR, Flatpak and Snap packages in one simple place.

mod app;
mod detail;
mod packages;
mod reload;
mod runner;
mod settings;
mod sources;
mod transaction;

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;

use qframe::runtime::Runtime;
use qframe::storage::Settings;
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
/// # Errors
///
/// Returns the terminal's error when the screen cannot be set up or drawn.
pub fn run() -> std::io::Result<()> {
    let settings = Settings::load("quvyta-packages").schema(settings::schema()).self_heal(true);
    let machine = app::Machine {
        dbpath: Path::new(LOCAL_DB),
        lock_dir: Path::new(PACMAN_DIR),
        lookup: Arc::new(on_path),
        runner: Arc::new(runner::Real),
        uid: current_uid(),
    };
    let app = app::Qpackages::new(machine, &settings);
    LOCALES
        .iter()
        .fold(Runtime::new(app), |runtime, (file, text)| runtime.locale_source(*file, *text))
        .keymap_source(KEYMAP.0, KEYMAP.1)
        .settings(&settings)
        .run()
}

/// The built-in files plus the compiled-in locales and keymap, as the runtime loads them, for
/// tests that drive the screen.
#[cfg(test)]
fn test_env() -> qframe::env::Env {
    let dirs = qframe::env::AssetDirs {
        locale_sources: LOCALES.iter().map(|(file, text)| ((*file).to_owned(), (*text).to_owned())).collect(),
        keymap_source: Some((KEYMAP.0.to_owned(), KEYMAP.1.to_owned())),
        ..qframe::env::AssetDirs::default()
    };
    qframe::env::Env::load(&dirs).expect("the locales and the keymap are readable")
}

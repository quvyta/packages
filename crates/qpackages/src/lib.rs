//! qpackages: pacman, AUR, Flatpak and Snap packages in one simple place.
//!
//! The program is [`run`]. The rest of what is public lets a test build the screen over a
//! pretend machine: [`app::Qpackages`] on an [`app::Machine`] whose programs answer through a
//! [`Runner`] and whose administrator helper is started by a [`Start`], drawn with the
//! language files and key bindings of [`asset_dirs`]. Nothing here reaches pacman or sudo
//! unless the machine given says so.

pub mod app;
pub mod autostart;
pub mod backend_settings;
mod build;
pub mod check;
mod detail;
mod helper;
mod icons;
mod installed;
pub mod ladder;
pub mod locales;
mod reload;
mod review;
mod runner;
mod settings;
mod settings_page;
mod snap;
mod sources;
mod store;
#[cfg(test)]
mod testing;
mod transaction;
mod updates;

pub use helper::session::{Connection, Host, Start};
pub use installed::Msg as InstalledMsg;
pub use locales::asset_dirs;
pub use reload::{Lookup, Snapshot};
pub use runner::{Output, Runner};
pub use settings_page::Msg as SettingsMsg;
pub use transaction::Msg as TransactionMsg;

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;

use qframe::i18n::I18n;
use qframe::runtime::Runtime;
use qframe::storage::Family;
use qframe::widgets::Appearance;
use qpackages_core::sources::on_path;

/// Where pacman keeps the records of installed packages.
const LOCAL_DB: &str = "/var/lib/pacman/local";

/// Where pacman keeps its lock file while a transaction runs.
const PACMAN_DIR: &str = "/var/lib/pacman";

/// Where pacman keeps the repository databases.
const SYNC_DB: &str = "/var/lib/pacman/sync";

/// Where the application menu's launchers are.
const APPLICATIONS: &str = "/usr/share/applications";

/// The folder, in qpac's cache folder, that holds the private copy of the repository databases
/// the update check refreshes.
const CHECK_DIR: &str = "check";

/// The compiled-in language files as a translator, for the parts that answer before or without a
/// screen: the shared preferences' language detection and `--check`'s report.
pub(crate) fn i18n() -> I18n {
    let mut i18n = I18n::builtin();
    for (file, text) in locales::LOCALES {
        i18n.add_source(file, text);
    }
    i18n
}

/// The user id this process runs as, read from the owner of its own `/proc` entry: root when
/// started with `sudo`, which the screen warns about. `None` when `/proc` cannot be read.
fn current_uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|metadata| metadata.uid())
}

/// Starts the application on the terminal: loads the settings, the compiled-in language files
/// and key bindings, and runs the screen until the user quits.
///
/// Started with `--privileged-helper` as its first argument, the program is the root helper
/// instead: it draws nothing and answers requests on its standard input until that ends. Started
/// with `--elevate-shim`, it is what paru and yay call in place of sudo while qpac builds an AUR
/// package: it passes one pacman call to the qpac running the build and exits as pacman would.
///
/// Whatever the settings reported when they were read (a file left in the old folder, a value
/// that was repaired) is written to standard error once the screen is gone, where the user reads
/// it after quitting.
///
/// # Errors
///
/// Returns the terminal's error when the screen cannot be set up or drawn, or the helper's when
/// its input or output fails.
pub fn run() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args_os().skip(1).map(|arg| arg.to_string_lossy().into_owned()).collect();
    if qpackages_core::build::shim::is_bare_validate(&args) {
        return Ok(());
    }
    if let Some((first, rest)) = args.split_first() {
        if first == qpackages_core::helper::FLAG {
            return helper::root::run(rest);
        }
        if first == qpackages_core::build::shim::FLAG {
            std::process::exit(build::shim::run(rest));
        }
    }
    let settings = settings::load();
    let family = Family::QUVYTA;
    // Read after the settings, which bring over an older file first: someone who used an earlier
    // release has a `packages.conf` by now and is not asked.
    let first_run = family.config_dir().and_then(|folder| app::FirstRun::in_folder(&folder));
    // While the wizard is to open nothing may be written, not even the family's shared file, so
    // the preferences are the ones it resolved without saving.
    let preferences = match &first_run {
        Some(first_run) => first_run.preferences().clone(),
        None => family.preferences(settings::APP, &i18n()),
    };
    let check_dir = family.cache_dir(settings::APP).map(|cache| cache.join(CHECK_DIR));
    let machine = app::Machine {
        dbpath: Path::new(LOCAL_DB),
        sync_dir: Path::new(SYNC_DB),
        applications: Path::new(APPLICATIONS),
        check_dir: check_dir.as_deref(),
        lock_dir: Path::new(PACMAN_DIR),
        lookup: Arc::new(on_path),
        runner: Arc::new(runner::Real),
        helper: Arc::new(helper::session::sudo),
        uid: current_uid(),
        utc_offset: qframe::date::local_offset_minutes(),
        app_catalog: Path::new(store::SWCATALOG),
        flatpak_catalogs: &store::flatpak_catalogs(),
        appearance: Appearance::new(family, settings::APP, preferences.clone()),
        snap_socket: Path::new(qpackages_core::snap::SOCKET),
        first_run,
    };
    let app = app::Qpackages::new(machine, &settings).with_self_update(app::SelfUpdateFolders::here());
    let result = locales::LOCALES
        .iter()
        .fold(Runtime::new(app), |runtime, (file, text)| runtime.locale_source(*file, *text))
        .keymap_source(locales::KEYMAP.0, locales::KEYMAP.1)
        .icon_source(icons::SET.0, icons::SET.1)
        .settings(&settings)
        .preferences(&preferences)
        .run();
    for diagnostic in settings.diagnostics() {
        eprintln!("{diagnostic}");
    }
    for diagnostic in preferences.diagnostics() {
        eprintln!("{diagnostic}");
    }
    for diagnostic in icons::diagnostics() {
        eprintln!("{diagnostic}");
    }
    result
}

/// The appearance rows of the settings page with `folder` as the family's settings folder, for
/// building the screen outside a terminal: a test or a picture then never reads or writes the
/// user's own settings. [`run`] uses the user's own family folder.
#[must_use]
pub fn appearance_in(folder: &Path) -> Appearance {
    let family = Family::QUVYTA;
    let preferences = family.preferences_in(folder, settings::APP, &i18n());
    Appearance::new(family, settings::APP, preferences).in_folder(folder)
}

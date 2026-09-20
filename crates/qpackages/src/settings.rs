//! The settings qpackages reads, where they live, and how the framework checks and repairs them.
//!
//! The file is `packages.conf` in the Quvyta family's folder, next to the other applications of
//! the family; any other configuration file goes into the `packages/` folder beside it.
//!
//! ```text
//! ~/.config/quvyta/
//!     packages.conf   qpackages's settings
//!     packages/       its other configuration files
//! ```
//!
//! Releases up to 0.1.1 kept `settings.toml` in `~/.config/quvyta-packages/`. That folder is
//! brought over once, at start, before the settings are read; a file that cannot be moved without
//! overwriting something stays whole where it is and is reported.
//!
//! Only keys the application actually reads are declared: a key nobody reads would be checked,
//! healed and written for nothing.

use std::path::Path;

use qframe::storage::{Family, Schema, Settings, config_dir};
use qpackages_core::sources::{AurPreference, Source};

use crate::sources;

/// The application's id in the family: its settings file is `packages.conf` and its other files
/// are under `packages/`.
pub const APP: &str = "packages";

/// The folder under the platform's config directory that held `settings.toml` before.
const LEGACY: &str = "quvyta-packages";

/// The key choosing which AUR helper drives the AUR when both are installed.
const AUR_HELPER: &str = "aur.helper";

/// The key choosing which program asks for administrator permission.
const PRIVILEGE_TOOL: &str = "privilege.tool";

/// Which program asks for administrator permission before the root helper starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeTool {
    /// pkexec when polkit is installed, sudo otherwise.
    Auto,
    /// Always pkexec: polkit asks, in a desktop window or on the terminal.
    Pkexec,
    /// Always sudo, on the terminal.
    Sudo,
}

/// The settings file's keys and what they accept.
///
/// `sources.<name>` turns a source on or off, all on by default. `aur.helper`
/// is `auto`, `paru` or `yay`; `auto` takes paru when it is there. `privilege.tool` is `auto`,
/// `pkexec` or `sudo`; `auto` takes pkexec when it is there.
#[must_use]
pub fn schema() -> Schema {
    let schema = sources::ALL
        .iter()
        .fold(Schema::builtin(), |schema, source| schema.flag(&source_key(*source), true))
        .choice(AUR_HELPER, ["auto", "paru", "yay"], "auto")
        .choice(PRIVILEGE_TOOL, ["auto", "pkexec", "sudo"], "auto");
    crate::backend_settings::declare(schema)
}

/// Whether the user keeps `source` turned on.
#[must_use]
pub fn source_enabled(settings: &Settings, source: Source) -> bool {
    settings.get_or(&source_key(source), true)
}

/// Which AUR helper the user prefers.
#[must_use]
pub fn aur_preference(settings: &Settings) -> AurPreference {
    match settings.get::<String>(AUR_HELPER).as_deref() {
        Some("paru") => AurPreference::Paru,
        Some("yay") => AurPreference::Yay,
        _ => AurPreference::Auto,
    }
}

/// Which program the user wants to ask for administrator permission.
#[must_use]
pub fn privilege_tool(settings: &Settings) -> PrivilegeTool {
    match settings.get::<String>(PRIVILEGE_TOOL).as_deref() {
        Some("pkexec") => PrivilegeTool::Pkexec,
        Some("sudo") => PrivilegeTool::Sudo,
        _ => PrivilegeTool::Auto,
    }
}

/// The AUR helper choices in the order the settings page offers them, each with the value the
/// file keeps.
pub const AUR_HELPERS: [(AurPreference, &str); 3] =
    [(AurPreference::Auto, "auto"), (AurPreference::Paru, "paru"), (AurPreference::Yay, "yay")];

/// The permission programs in the order the settings page offers them, each with the value the
/// file keeps.
pub const PRIVILEGE_TOOLS: [(PrivilegeTool, &str); 3] =
    [(PrivilegeTool::Auto, "auto"), (PrivilegeTool::Pkexec, "pkexec"), (PrivilegeTool::Sudo, "sudo")];

/// Turns `source` on or off and says whether that changed anything. Only the value changes;
/// saving is the caller's, so a change made on the settings page is written once, in the
/// background.
pub fn set_source_enabled(settings: &mut Settings, source: Source, enabled: bool) -> bool {
    settings.set(&source_key(source), enabled)
}

/// Chooses the AUR helper to prefer and says whether that changed anything.
pub fn set_aur_preference(settings: &mut Settings, preference: AurPreference) -> bool {
    AUR_HELPERS
        .iter()
        .find(|(choice, _)| *choice == preference)
        .is_some_and(|(_, value)| settings.set(AUR_HELPER, (*value).to_owned()))
}

/// Chooses the program that asks for administrator permission and says whether that changed
/// anything.
pub fn set_privilege_tool(settings: &mut Settings, tool: PrivilegeTool) -> bool {
    PRIVILEGE_TOOLS
        .iter()
        .find(|(choice, _)| *choice == tool)
        .is_some_and(|(_, value)| settings.set(PRIVILEGE_TOOL, (*value).to_owned()))
}

/// Brings the settings over from the folder earlier releases used and reads them from the
/// family's folder, checked and healed. What could not be moved is among the settings'
/// diagnostics, first. Without a home folder the settings stay in memory and say why.
#[must_use]
pub fn load() -> Settings {
    match (Family::QUVYTA.config_dir(), config_dir(LEGACY)) {
        (Some(folder), Some(legacy)) => load_in(&folder, &legacy),
        _ => checked(Settings::load_member(&Family::QUVYTA, APP)),
    }
}

/// [`load`] with `folder` as the family's folder and `legacy` as the folder earlier releases
/// used, so a test never touches the user's own settings.
///
/// The framework copies each file, compares the copy and only then removes the original; a file
/// whose new place is taken is left alone, so an existing `packages.conf` wins and the old
/// `settings.toml` stays whole beside it. A missing old folder is nothing to do.
#[must_use]
pub fn load_in(folder: &Path, legacy: &Path) -> Settings {
    let moved = Family::QUVYTA.adopt_in(folder, APP, legacy);
    checked(Settings::open(folder.join(format!("{APP}.conf"))).member_of(&Family::QUVYTA))
        .with_diagnostics(moved.diagnostics().to_vec())
}

/// `settings` checked against qpackages's keys and healed, with a backup of what the user wrote.
fn checked(settings: Settings) -> Settings {
    settings.schema(schema()).self_heal(true)
}

fn source_key(source: Source) -> String {
    format!("sources.{}", sources::name(source))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// A fresh folder under the system's temporary folder; never the user's own config folder.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-test-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("folder");
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        fs::write(path, text).expect("file");
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("readable")
    }

    const YAY: &str = "[aur]\nhelper = \"yay\"\n";

    #[test]
    fn the_old_folder_moves_whole_into_the_family_and_is_removed() {
        let root = temp("move");
        let folder = root.join("quvyta");
        let legacy = root.join("quvyta-packages");
        write(&legacy.join("settings.toml"), YAY);
        write(&legacy.join("settings.toml.bak"), "old backup\n");

        let settings = load_in(&folder, &legacy);

        assert!(settings.diagnostics().is_empty(), "{:?}", settings.diagnostics());
        assert_eq!(read(&folder.join("packages.conf")), YAY);
        assert_eq!(read(&folder.join("packages").join("settings.toml.bak")), "old backup\n");
        assert!(!legacy.exists(), "the emptied old folder is removed");
        assert_eq!(aur_preference(&settings), AurPreference::Yay);
        assert_eq!(settings.path(), Some(folder.join("packages.conf").as_path()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_start_moves_nothing_and_reads_the_same() {
        let root = temp("again");
        let folder = root.join("quvyta");
        let legacy = root.join("quvyta-packages");
        write(&legacy.join("settings.toml"), YAY);
        let _ = load_in(&folder, &legacy);

        let settings = load_in(&folder, &legacy);

        assert!(settings.diagnostics().is_empty(), "{:?}", settings.diagnostics());
        assert_eq!(read(&folder.join("packages.conf")), YAY);
        assert_eq!(aur_preference(&settings), AurPreference::Yay);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_existing_packages_conf_wins_and_the_old_file_stays_whole_and_is_reported() {
        let root = temp("both");
        let folder = root.join("quvyta");
        let legacy = root.join("quvyta-packages");
        write(&legacy.join("settings.toml"), YAY);
        write(&folder.join("packages.conf"), "[aur]\nhelper = \"paru\"\n");

        let settings = load_in(&folder, &legacy);

        assert_eq!(read(&legacy.join("settings.toml")), YAY, "the old file is never touched");
        assert_eq!(read(&folder.join("packages.conf")), "[aur]\nhelper = \"paru\"\n");
        assert_eq!(aur_preference(&settings), AurPreference::Paru);
        assert_eq!(settings.diagnostics().len(), 1, "{:?}", settings.diagnostics());
        assert!(settings.diagnostics()[0].message.contains("settings.toml"), "{}", settings.diagnostics()[0]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fresh_start_writes_nothing_until_a_setting_changes() {
        let root = temp("fresh");
        let folder = root.join("quvyta");
        let legacy = root.join("quvyta-packages");

        let mut settings = load_in(&folder, &legacy);

        assert!(settings.diagnostics().is_empty(), "{:?}", settings.diagnostics());
        assert!(!folder.join("packages.conf").exists());
        assert!(!legacy.exists());
        settings.set(AUR_HELPER, "yay".to_owned());
        settings.save().expect("saved");
        assert_eq!(read(&folder.join("packages.conf")), YAY);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_damaged_old_file_is_moved_as_it_was_then_healed_with_a_backup() {
        let root = temp("damaged");
        let folder = root.join("quvyta");
        let legacy = root.join("quvyta-packages");
        let text = "[aur]\nhelper = \"trizen\"\n";
        write(&legacy.join("settings.toml"), text);

        let settings = load_in(&folder, &legacy);

        assert_eq!(read(&folder.join("packages.conf.bak")), text, "what the user wrote is kept");
        assert!(!settings.diagnostics().is_empty());
        assert_eq!(aur_preference(&settings), AurPreference::Auto);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn every_source_is_on_and_the_helper_is_automatic_by_default() {
        let settings = Settings::parse_str("settings.toml", "").schema(schema());
        for source in sources::ALL {
            assert!(source_enabled(&settings, source), "{source:?}");
        }
        assert_eq!(aur_preference(&settings), AurPreference::Auto);
        assert_eq!(privilege_tool(&settings), PrivilegeTool::Auto);
    }

    #[test]
    fn the_file_picks_the_program_that_asks_for_permission() {
        for (value, tool) in
            [("auto", PrivilegeTool::Auto), ("pkexec", PrivilegeTool::Pkexec), ("sudo", PrivilegeTool::Sudo)]
        {
            let text = format!("[privilege]\ntool = \"{value}\"\n");
            let settings = Settings::parse_str("settings.toml", &text).schema(schema());
            assert_eq!(settings.diagnostics(), &[], "{value}");
            assert_eq!(privilege_tool(&settings), tool, "{value}");
        }
    }

    #[test]
    fn healing_replaces_a_permission_program_nobody_knows() {
        let text = "[privilege]\ntool = \"doas\"\n";
        let settings = Settings::parse_str("settings.toml", text).schema(schema()).self_heal(true);
        assert_eq!(privilege_tool(&settings), PrivilegeTool::Auto);
        assert_eq!(settings.diagnostics().len(), 1, "the repair is reported: {:?}", settings.diagnostics());
        assert!(settings.diagnostics()[0].to_string().contains("doas"), "{}", settings.diagnostics()[0]);
    }

    #[test]
    fn the_file_turns_a_source_off_and_picks_a_helper() {
        let text = "[sources]\nsnap = false\n\n[aur]\nhelper = \"yay\"\n";
        let settings = Settings::parse_str("settings.toml", text).schema(schema());
        assert_eq!(settings.diagnostics(), &[]);
        assert!(!source_enabled(&settings, Source::Snap));
        assert!(source_enabled(&settings, Source::Pacman));
        assert_eq!(aur_preference(&settings), AurPreference::Yay);
    }

    #[test]
    fn what_the_settings_page_sets_reads_back_and_is_written_as_the_file_expects() {
        let mut settings = Settings::parse_str("settings.toml", "").schema(schema());
        assert!(set_source_enabled(&mut settings, Source::Flatpak, false));
        assert!(set_aur_preference(&mut settings, AurPreference::Yay));
        assert!(set_privilege_tool(&mut settings, PrivilegeTool::Sudo));
        assert!(!set_privilege_tool(&mut settings, PrivilegeTool::Sudo), "the same value changes nothing");
        assert!(!source_enabled(&settings, Source::Flatpak));
        assert_eq!(aur_preference(&settings), AurPreference::Yay);
        assert_eq!(privilege_tool(&settings), PrivilegeTool::Sudo);
        let reread = Settings::parse_str("settings.toml", &settings.to_toml()).schema(schema());
        assert_eq!(reread.diagnostics(), &[], "{}", settings.to_toml());
        assert!(!source_enabled(&reread, Source::Flatpak));
        assert_eq!(aur_preference(&reread), AurPreference::Yay);
        assert_eq!(privilege_tool(&reread), PrivilegeTool::Sudo);
    }

    #[test]
    fn healing_replaces_a_helper_nobody_knows_and_drops_an_unknown_key() {
        let text = "[aur]\nhelper = \"trizen\"\n\n[updates]\nmode = \"install\"\n";
        let settings = Settings::parse_str("settings.toml", text).schema(schema()).self_heal(true);
        assert_eq!(aur_preference(&settings), AurPreference::Auto);
        assert!(settings.value("updates.mode").is_none(), "a key this version does not read is removed");
        assert_eq!(settings.diagnostics().len(), 2, "each repair is reported");
    }
}

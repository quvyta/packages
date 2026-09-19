//! The settings of the background check and of snapshots before an update.
//!
//! ```toml
//! [updates]
//! autostart = "off"   # "on" runs `qpac --check` from a user timer
//! interval = 6        # hours between checks, 1 to 168
//!
//! [backup]
//! tool = "snapper"    # "off", "snapper" or "timeshift"; unset picks what is installed
//! ```
//!
//! [`declare`] adds these keys to the settings file's schema, so they are checked and healed
//! with the rest.

use qframe::storage::{Schema, SettingKind, Settings};
use qpackages_core::backup::{Detected, Setting};

/// Whether the background check runs.
pub const AUTOSTART: &str = "updates.autostart";

/// Hours between background checks.
pub const INTERVAL: &str = "updates.interval";

/// Which tool takes a snapshot before an update.
pub const BACKUP_TOOL: &str = "backup.tool";

/// The hours between checks when the user chose none: four checks a day.
pub const DEFAULT_INTERVAL: u32 = 6;

/// The allowed hours between checks: never more often than hourly, never less than weekly.
const INTERVALS: std::ops::RangeInclusive<i64> = 1..=168;

/// `schema` with the keys above: `updates.autostart` is `off` or `on`, off by default;
/// `updates.interval` a whole number of hours from 1 to 168, 6 by default; `backup.tool` `off`,
/// `snapper` or `timeshift`, with no default, since the default depends on what is installed.
#[must_use]
pub fn declare(schema: Schema) -> Schema {
    schema
        .choice(AUTOSTART, ["off", "on"], "off")
        .check::<i64>(INTERVAL, i64::from(DEFAULT_INTERVAL), |hours| INTERVALS.contains(hours))
        .optional(BACKUP_TOOL, SettingKind::choice(["off", "snapper", "timeshift"]))
}

/// Whether the user turned the background check on.
#[must_use]
pub fn autostart(settings: &Settings) -> bool {
    settings.get::<String>(AUTOSTART).as_deref() == Some("on")
}

/// The hours between background checks, held to 1–168.
#[must_use]
pub fn interval_hours(settings: &Settings) -> u32 {
    settings
        .get::<i64>(INTERVAL)
        .map(|hours| hours.clamp(*INTERVALS.start(), *INTERVALS.end()))
        .and_then(|hours| u32::try_from(hours).ok())
        .unwrap_or(DEFAULT_INTERVAL)
}

/// The snapshot setting: the user's choice, or, when there is none, the tool installed on the
/// machine `found` describes.
#[must_use]
pub fn backup_tool(settings: &Settings, found: &Detected) -> Setting {
    settings
        .get::<String>(BACKUP_TOOL)
        .as_deref()
        .and_then(Setting::parse)
        .unwrap_or_else(|| Setting::default_for(found))
}

#[cfg(test)]
mod tests {
    use qpackages_core::backup::Tool;

    use super::*;

    fn read(text: &str) -> Settings {
        Settings::parse_str("packages.conf", text).schema(declare(Schema::builtin())).self_heal(true)
    }

    #[test]
    fn nothing_written_means_off_every_six_hours_and_whatever_is_installed() {
        let settings = read("");
        assert!(!autostart(&settings));
        assert_eq!(interval_hours(&settings), 6);
        let timeshift = Detected { timeshift: true, ..Detected::default() };
        assert_eq!(backup_tool(&settings, &timeshift), Setting::Tool(Tool::Timeshift));
        assert_eq!(backup_tool(&settings, &Detected::default()), Setting::Off);
    }

    #[test]
    fn the_file_turns_the_check_on_and_picks_its_interval_and_tool() {
        let settings = read("[updates]\nautostart = \"on\"\ninterval = 24\n\n[backup]\ntool = \"off\"\n");
        assert!(autostart(&settings));
        assert_eq!(interval_hours(&settings), 24);
        let snapper = Detected { snapper: true, snapper_root: Some(true), ..Detected::default() };
        assert_eq!(backup_tool(&settings, &snapper), Setting::Off, "a choice wins over what is installed");
    }

    #[test]
    fn values_out_of_range_are_healed() {
        let settings = read("[updates]\nautostart = \"yes\"\ninterval = 0\n\n[backup]\ntool = \"btrfs\"\n");
        assert!(!autostart(&settings));
        assert_eq!(interval_hours(&settings), 6);
        assert_eq!(settings.get::<String>(BACKUP_TOOL), None, "an unknown tool is removed, not guessed");
        assert_eq!(interval_hours(&read("[updates]\ninterval = 9999\n")), 6);
    }

    #[test]
    fn an_unchecked_file_is_still_read_safely() {
        let settings = Settings::parse_str("packages.conf", "[updates]\ninterval = 9999\nautostart = true\n");
        assert_eq!(interval_hours(&settings), 168);
        assert!(!autostart(&settings));
    }
}

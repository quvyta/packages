//! The settings of the background check and of snapshots before an update.
//!
//! ```toml
//! [updates]
//! autostart = "off"   # "on" runs `qpac --check` from a user timer
//! interval = 6        # hours between checks, 1 to 168
//! mode = "notify"     # "notify", "download" or "install": what the check does with what it finds
//!
//! [backup]
//! tool = "snapper"    # "off", "snapper" or "timeshift"; unset picks what is installed
//!
//! [cleanup]
//! orphans = "ask"     # "never", "ask" or "auto": what happens to orphans after a change
//!
//! [mirrors]
//! countries = ["TR", "DE"]  # reflector's country codes; none for every country
//! count = 10                # the most recently synchronized mirrors kept, 1 to 100
//! age = 12                  # hours since a mirror last synchronized, 1 to 720
//! sort = "rate"             # "rate", "age", "score" or "delay"
//! ```
//!
//! [`declare`] adds these keys to the settings file's schema, so they are checked and healed
//! with the rest.

use qframe::storage::{Schema, SettingKind, Settings};
use qpackages_core::backup::{Detected, Setting, Tool};
use qpackages_core::reflector::{Country, MirrorError, Mirrors, Protocol, Sort, is_country_code};

use crate::ladder::Mode;

/// Whether the background check runs.
pub const AUTOSTART: &str = "updates.autostart";

/// Hours between background checks.
pub const INTERVAL: &str = "updates.interval";

/// What the background check does with the updates it finds: the ladder's step.
pub const MODE: &str = "updates.mode";

/// Which tool takes a snapshot before an update.
pub const BACKUP_TOOL: &str = "backup.tool";

/// What happens to the orphans a removal or an update leaves.
pub const ORPHANS: &str = "cleanup.orphans";

/// What happens to the orphans a removal or an update leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orphans {
    /// Nothing is said; the Installed tab still marks them.
    Never,
    /// A notice says how many are left and offers to clean them up.
    Ask,
    /// They are removed at the end, without asking again.
    Auto,
}

impl Orphans {
    /// The choices in the order the settings page offers them, each with the value the file
    /// keeps.
    pub const ALL: [(Self, &'static str); 3] = [(Self::Never, "never"), (Self::Ask, "ask"), (Self::Auto, "auto")];
}

/// The countries reflector takes mirrors from.
pub const MIRROR_COUNTRIES: &str = "mirrors.countries";

/// How many of the most recently synchronized mirrors reflector keeps.
pub const MIRROR_COUNT: &str = "mirrors.count";

/// How recently, in hours, a mirror must have synchronized.
pub const MIRROR_AGE: &str = "mirrors.age";

/// How reflector orders the mirrors.
pub const MIRROR_SORT: &str = "mirrors.sort";

/// The intervals the settings page offers, in hours: from hourly, the least the mirrors are
/// asked, to weekly.
pub const OFFERED_INTERVALS: [u32; 6] = [1, 3, 6, 12, 24, 168];

/// The mirror counts the settings page offers.
pub const OFFERED_COUNTS: [u8; 4] = [5, 10, 20, 50];

/// The ages the settings page offers, in hours.
pub const OFFERED_AGES: [u16; 4] = [6, 12, 24, 48];

/// The orders the settings page offers.
pub const OFFERED_SORTS: [Sort; 4] = [Sort::Rate, Sort::Age, Sort::Score, Sort::Delay];

/// The hours between checks when the user chose none: four checks a day.
pub const DEFAULT_INTERVAL: u32 = 6;

/// The allowed hours between checks: never more often than hourly, never less than weekly.
const INTERVALS: std::ops::RangeInclusive<i64> = 1..=168;

/// `schema` with the keys above: `updates.autostart` is `off` or `on`, off by default;
/// `updates.interval` a whole number of hours from 1 to 168, 6 by default; `updates.mode`
/// `notify`, `download` or `install`, `notify` by default; `backup.tool` `off`,
/// `snapper` or `timeshift`, with no default, since the default depends on what is installed;
/// `cleanup.orphans` `never`, `ask` or `auto`, `ask` by default; the `mirrors.*` keys as reflector
/// accepts them, every country, 10 mirrors, 12 hours and by speed by default.
#[must_use]
pub fn declare(schema: Schema) -> Schema {
    schema
        .choice(AUTOSTART, ["off", "on"], "off")
        .check::<i64>(INTERVAL, i64::from(DEFAULT_INTERVAL), |hours| INTERVALS.contains(hours))
        .choice(MODE, Mode::ALL.map(|(_, key)| key), "notify")
        .optional(BACKUP_TOOL, SettingKind::choice(["off", "snapper", "timeshift"]))
        .choice(ORPHANS, Orphans::ALL.map(|(_, value)| value), "ask")
        .check::<Vec<String>>(MIRROR_COUNTRIES, Vec::new(), |codes| {
            codes.len() <= 64 && codes.iter().all(|code| is_country_code(code))
        })
        .check::<i64>(MIRROR_COUNT, 10, |count| (1..=100).contains(count))
        .check::<i64>(MIRROR_AGE, 12, |hours| (1..=720).contains(hours))
        .choice(MIRROR_SORT, OFFERED_SORTS.map(Sort::key), "rate")
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

/// The ladder's step the user chose; the lowest, telling, when nothing readable is written. What
/// runs on this machine is [`Mode::effective`] of it.
#[must_use]
pub fn update_mode(settings: &Settings) -> Mode {
    settings.get::<String>(MODE).as_deref().and_then(Mode::parse).unwrap_or(Mode::Notify)
}

/// Chooses the ladder's step and says whether that changed anything.
pub fn set_update_mode(settings: &mut Settings, mode: Mode) -> bool {
    settings.set(MODE, mode.key().to_owned())
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

/// What the user wants done with orphans; asked when nothing readable is written.
#[must_use]
pub fn orphans(settings: &Settings) -> Orphans {
    let value = settings.get::<String>(ORPHANS);
    Orphans::ALL.iter().find(|(_, key)| Some(*key) == value.as_deref()).map_or(Orphans::Ask, |(choice, _)| *choice)
}

/// Chooses what happens to orphans and says whether that changed anything.
pub fn set_orphans(settings: &mut Settings, choice: Orphans) -> bool {
    Orphans::ALL
        .iter()
        .find(|(known, _)| *known == choice)
        .is_some_and(|(_, value)| settings.set(ORPHANS, (*value).to_owned()))
}

/// Turns the background check on or off and says whether that changed anything.
pub fn set_autostart(settings: &mut Settings, on: bool) -> bool {
    settings.set(AUTOSTART, (if on { "on" } else { "off" }).to_owned())
}

/// Sets the hours between background checks and says whether that changed anything.
pub fn set_interval(settings: &mut Settings, hours: u32) -> bool {
    settings.set(INTERVAL, i64::from(hours.clamp(1, 168)))
}

/// Chooses the snapshot tool and says whether that changed anything.
pub fn set_backup_tool(settings: &mut Settings, setting: Setting) -> bool {
    settings.set(BACKUP_TOOL, setting.key().to_owned())
}

/// The snapshot choices this machine can take: off, and each tool that is installed and, for
/// snapper, set up for the root file system as far as can be told.
#[must_use]
pub fn backup_choices(found: &Detected) -> Vec<Setting> {
    let mut choices = vec![Setting::Off];
    if found.snapper && found.snapper_root != Some(false) {
        choices.push(Setting::Tool(Tool::Snapper));
    }
    if found.timeshift {
        choices.push(Setting::Tool(Tool::Timeshift));
    }
    choices
}

/// The country codes reflector takes mirrors from; empty for every country.
#[must_use]
pub fn mirror_countries(settings: &Settings) -> Vec<String> {
    settings.get::<Vec<String>>(MIRROR_COUNTRIES).unwrap_or_default()
}

/// Adds `code` to the countries, or takes it out when it is there, and says whether that changed
/// anything: a code that is not two capital letters changes nothing.
pub fn toggle_mirror_country(settings: &mut Settings, code: &str) -> bool {
    if !is_country_code(code) {
        return false;
    }
    let mut codes = mirror_countries(settings);
    match codes.iter().position(|known| known == code) {
        Some(index) => {
            codes.remove(index);
        }
        None => codes.push(code.to_owned()),
    }
    settings.set(MIRROR_COUNTRIES, codes)
}

/// How many mirrors reflector keeps.
#[must_use]
pub fn mirror_count(settings: &Settings) -> u8 {
    settings.get::<i64>(MIRROR_COUNT).and_then(|count| u8::try_from(count.clamp(1, 100)).ok()).unwrap_or(10)
}

/// How recently, in hours, a mirror must have synchronized.
#[must_use]
pub fn mirror_age(settings: &Settings) -> u16 {
    settings.get::<i64>(MIRROR_AGE).and_then(|hours| u16::try_from(hours.clamp(1, 720)).ok()).unwrap_or(12)
}

/// How reflector orders the mirrors.
#[must_use]
pub fn mirror_sort(settings: &Settings) -> Sort {
    settings.get::<String>(MIRROR_SORT).as_deref().and_then(Sort::parse).unwrap_or_default()
}

/// Sets how many mirrors reflector keeps and says whether that changed anything.
pub fn set_mirror_count(settings: &mut Settings, count: u8) -> bool {
    settings.set(MIRROR_COUNT, i64::from(count))
}

/// Sets how recently a mirror must have synchronized and says whether that changed anything.
pub fn set_mirror_age(settings: &mut Settings, hours: u16) -> bool {
    settings.set(MIRROR_AGE, i64::from(hours))
}

/// Sets how reflector orders the mirrors and says whether that changed anything.
pub fn set_mirror_sort(settings: &mut Settings, sort: Sort) -> bool {
    settings.set(MIRROR_SORT, sort.key().to_owned())
}

/// The mirror setting the helper is asked to apply, checked against the countries reflector
/// lists in `known`. Mirrors are always reached over https: the packages are signed either way,
/// but http would show everyone on the way what is installed.
///
/// # Errors
///
/// Returns the first value reflector would not accept, such as a country it no longer lists.
pub fn mirrors(settings: &Settings, known: &[Country]) -> Result<Mirrors, MirrorError> {
    let countries = mirror_countries(settings);
    let countries: Vec<&str> = countries.iter().map(String::as_str).collect();
    Mirrors::new(
        &countries,
        known,
        Protocol::Https,
        mirror_age(settings),
        mirror_count(settings),
        mirror_sort(settings),
    )
}

#[cfg(test)]
mod tests {
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
        assert_eq!(orphans(&settings), Orphans::Ask);
        assert_eq!(update_mode(&settings), Mode::Notify);
    }

    #[test]
    fn the_ladder_step_is_read_healed_and_written() {
        assert_eq!(update_mode(&read("[updates]\nmode = \"download\"\n")), Mode::Download);
        assert_eq!(update_mode(&read("[updates]\nmode = \"install\"\n")), Mode::Install);
        let mut healed = read("[updates]\nmode = \"always\"\n");
        assert_eq!(update_mode(&healed), Mode::Notify, "an unknown step is the lowest");
        assert!(set_update_mode(&mut healed, Mode::Download));
        assert!(!set_update_mode(&mut healed, Mode::Download), "the same step changes nothing");
        assert_eq!(update_mode(&healed), Mode::Download);
    }

    #[test]
    fn the_mirror_settings_default_to_every_country_and_are_checked() {
        let settings = read("");
        let every = mirrors(&settings, &[]).expect("the defaults are valid");
        assert_eq!(every.values(), ["https", "12", "10", "rate"]);
        let known = [Country { name: "Turkey".to_owned(), code: "TR".to_owned(), mirrors: 7 }];
        let mut chosen = read("[mirrors]\ncountries = [\"TR\"]\ncount = 20\nage = 24\nsort = \"score\"\n");
        assert_eq!(mirrors(&chosen, &known).expect("valid").values(), ["https", "24", "20", "score", "TR"]);
        assert!(mirrors(&chosen, &[]).is_err(), "a country reflector does not list is refused");
        assert!(toggle_mirror_country(&mut chosen, "TR"));
        assert_eq!(mirror_countries(&chosen), Vec::<String>::new());
        assert!(!toggle_mirror_country(&mut chosen, "tr; rm"), "only a code is ever written");
        let healed = read("[mirrors]\ncountries = [\"xx\"]\ncount = 0\nage = 9999\nsort = \"random\"\n");
        assert_eq!(mirrors(&healed, &[]).expect("healed to the defaults").values(), ["https", "12", "10", "rate"]);
    }

    #[test]
    fn only_the_tools_that_are_there_are_offered() {
        assert_eq!(backup_choices(&Detected::default()), [Setting::Off]);
        let both = Detected { snapper: true, snapper_root: None, timeshift: true, snap_pac: false };
        assert_eq!(
            backup_choices(&both),
            [Setting::Off, Setting::Tool(Tool::Snapper), Setting::Tool(Tool::Timeshift)],
            "snapper's configuration may not be readable; it is not ruled out for that"
        );
        let unconfigured = Detected { snapper: true, snapper_root: Some(false), ..Detected::default() };
        assert_eq!(backup_choices(&unconfigured), [Setting::Off]);
    }

    #[test]
    fn the_orphan_choice_is_read_healed_and_written() {
        assert_eq!(orphans(&read("[cleanup]\norphans = \"auto\"\n")), Orphans::Auto);
        assert_eq!(orphans(&read("[cleanup]\norphans = \"never\"\n")), Orphans::Never);
        let mut healed = read("[cleanup]\norphans = \"always\"\n");
        assert_eq!(orphans(&healed), Orphans::Ask, "an unknown value is asked about");
        assert!(set_orphans(&mut healed, Orphans::Never));
        assert!(!set_orphans(&mut healed, Orphans::Never), "the same choice changes nothing");
        assert_eq!(orphans(&healed), Orphans::Never);
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

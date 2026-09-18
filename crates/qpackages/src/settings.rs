//! The settings qpackages reads, described so the framework can check and repair the file.
//!
//! Only keys the application actually reads are declared: a key nobody reads would be checked,
//! healed and written for nothing.

use qframe::storage::{Schema, Settings};
use qpackages_core::sources::{AurPreference, Source};

use crate::sources;

/// The key choosing which AUR helper drives the AUR when both are installed.
const AUR_HELPER: &str = "aur.helper";

/// The settings file's keys and what they accept.
///
/// `sources.<name>` turns a source on or off in the sidebar, all on by default. `aur.helper`
/// is `auto`, `paru` or `yay`; `auto` takes paru when it is there.
#[must_use]
pub fn schema() -> Schema {
    sources::ALL.iter().fold(Schema::builtin(), |schema, source| schema.flag(&source_key(*source), true)).choice(
        AUR_HELPER,
        ["auto", "paru", "yay"],
        "auto",
    )
}

/// Whether the user keeps `source` in the sidebar.
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

fn source_key(source: Source) -> String {
    format!("sources.{}", sources::name(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_is_on_and_the_helper_is_automatic_by_default() {
        let settings = Settings::parse_str("settings.toml", "").schema(schema());
        for source in sources::ALL {
            assert!(source_enabled(&settings, source), "{source:?}");
        }
        assert_eq!(aur_preference(&settings), AurPreference::Auto);
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
    fn healing_replaces_a_helper_nobody_knows_and_drops_an_unknown_key() {
        let text = "[aur]\nhelper = \"trizen\"\n\n[updates]\nmode = \"install\"\n";
        let settings = Settings::parse_str("settings.toml", text).schema(schema()).self_heal(true);
        assert_eq!(aur_preference(&settings), AurPreference::Auto);
        assert!(settings.value("updates.mode").is_none(), "a key this version does not read is removed");
        assert_eq!(settings.diagnostics().len(), 2, "each repair is reported");
    }
}

//! The settings page on a pretend machine with pacman, paru and fakeroot, whose settings file
//! lives in its own temporary folder.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;

use super::*;
use crate::app::{Msg as AppMsg, Qpackages};
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in};

/// The application with its settings page open, and where its settings file is.
fn page(width: u16, height: u16) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("settings", &[Sample::new("bash", "5.3-1", "Shell")]);
    let recorded = Arc::new(Recorded::default());
    let settings = Settings::open(scratch.root().join("packages.conf"));
    let mut h = Harness::with_env(app_in(&scratch, settings, &recorded), crate::test_env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.send(AppMsg::OpenSettings);
    (h, scratch, recorded)
}

/// What the family's shared file says.
fn shared_file(scratch: &Scratch) -> String {
    fs::read_to_string(scratch.root().join("quvyta.conf")).unwrap_or_default()
}

/// What the settings file says after the background save ran.
fn written(h: &mut Harness<Qpackages>, scratch: &Scratch) -> String {
    h.advance(Duration::from_millis(20));
    fs::read_to_string(scratch.root().join("packages.conf")).unwrap_or_default()
}

#[test]
fn the_page_lists_the_settings_that_exist_under_their_headings() {
    let (h, _scratch, _) = page(100, 80);
    let screen = h.screen();
    let headings = ["Sources", "\n  Updates\n", "Backup", "Cleanup", "Mirrors", "Permission", "Appearance"];
    let positions: Vec<usize> = headings.iter().map(|text| screen.find(text).expect("a heading")).collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{screen}");
    for text in [
        "AUR",
        "through paru",
        "Flatpak",
        "Install flatpak",
        "Snap",
        "not installed",
        "AUR helper",
        "Automatic",
        "Asked by",
        "sudo asks on the terminal.",
        "Language",
        "Theme",
        "Icons",
        "Reduce motion",
        "Pillar",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn a_source_turned_off_is_saved_at_once_and_nothing_is_written_before() {
    let (mut h, scratch, _) = page(100, 40);
    assert_eq!(written(&mut h, &scratch), "", "opening the page writes nothing");
    h.send(AppMsg::Settings(Msg::Source(Source::Flatpak, false)));
    assert!(written(&mut h, &scratch).contains("flatpak = false"), "{}", written(&mut h, &scratch));
}

#[test]
fn choosing_the_permission_program_takes_effect_and_is_saved() {
    let (mut h, scratch, _) = page(100, 40);
    h.send(AppMsg::Settings(Msg::PrivilegeTool(1)));
    let screen = h.screen();
    assert!(screen.contains("polkit asks, in a window or on the terminal."), "pkexec is used even unfound:\n{screen}");
    assert!(written(&mut h, &scratch).contains("tool = \"pkexec\""));
    h.send(AppMsg::Settings(Msg::PrivilegeTool(2)));
    assert!(h.screen().contains("sudo asks on the terminal."), "{}", h.screen());
    assert!(written(&mut h, &scratch).contains("tool = \"sudo\""));
}

#[test]
fn choosing_the_aur_helper_reads_the_sources_again() {
    let (mut h, scratch, recorded) = page(100, 40);
    let reads = |recorded: &Recorded| recorded.command_lines().iter().filter(|line| *line == "pacman -Qqm").count();
    let before = reads(&recorded);
    h.send(AppMsg::Settings(Msg::AurHelper(2)));
    assert!(written(&mut h, &scratch).contains("helper = \"yay\""));
    assert_eq!(reads(&recorded), before + 1, "the sources are looked for again");
    assert!(h.screen().contains("through paru"), "a preference only breaks a tie:\n{}", h.screen());
    h.send(AppMsg::Settings(Msg::AurHelper(2)));
    assert_eq!(reads(&recorded), before + 1, "the same choice changes nothing");
}

#[test]
fn an_appearance_choice_applies_and_goes_to_the_family_s_file() {
    let (mut h, scratch, _) = page(100, 40);
    h.send(AppMsg::Settings(Msg::Appearance(AppearanceChange::Icons(IconMode::Ascii))));
    h.send(AppMsg::Settings(Msg::Appearance(AppearanceChange::Language("tr".to_owned()))));
    assert!(h.screen().contains("Ayarlar"), "the language changed at once:\n{}", h.screen());
    // The rows open following the family, so the value goes to the shared file and qpac's own
    // file says that it follows.
    let shared = shared_file(&scratch);
    assert!(shared.contains("icons = \"ascii\"") && shared.contains("language = \"tr\""), "{shared}");
    let own = written(&mut h, &scratch);
    assert!(own.contains("icons = \"quvyta\"") && own.contains("language = \"quvyta\""), "{own}");
}

#[test]
fn a_choice_made_here_only_goes_to_qpac_s_own_file() {
    let (mut h, scratch, _) = page(100, 40);
    let icons = AppearanceChange::Everywhere(qframe::storage::Shared::Icons, false);
    h.send(AppMsg::Settings(Msg::Appearance(icons)));
    // What the family said before the change; the machine's own detection decides what that is.
    let before = shared_file(&scratch);
    h.send(AppMsg::Settings(Msg::Appearance(AppearanceChange::Icons(IconMode::Nerd))));
    let own = written(&mut h, &scratch);
    assert!(own.contains("icons = \"nerd\""), "{own}");
    assert_eq!(shared_file(&scratch), before, "the family is left alone");
}

#[test]
fn a_narrow_page_keeps_the_offers_and_the_rules() {
    for width in [40, 60] {
        let (mut h, _scratch, _) = page(width, 40);
        h.set_glyph_mode(GlyphMode::Ascii);
        let screen = h.screen();
        assert!(screen.contains("Install flatpak") || screen.contains("Install flat"), "{width}:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}:\n{screen}");
        }
    }
}

#[test]
fn a_page_taller_than_the_screen_stays_put_on_a_click_and_follows_the_keys() {
    let (mut h, _scratch, _) = page(100, 20);
    let sources = h.screen().find("Sources").expect("the first heading is on screen");
    let (x, y) = h.find("AUR helper").expect("a row of the first section");
    h.click(x, y);
    assert_eq!(h.screen().find("Sources"), Some(sources), "a click does not scroll:\n{}", h.screen());
    // Walking down far enough reaches the last section, which no screen this short can show at once.
    for _ in 0..40 {
        h.press("down");
    }
    let screen = h.screen();
    assert!(screen.contains("Pillar"), "the keys reached the last row:\n{screen}");
    assert!(!screen.contains("Sources"), "and the list scrolled to it:\n{screen}");
}

#[test]
fn the_appearance_rows_read_in_both_languages_and_offer_the_family() {
    let (mut h, _scratch, _) = page(100, 80);
    for (locale, heading, everywhere) in
        [("en", "Appearance", "In every Quvyta application"), ("tr", "Görünüm", "Tüm Quvyta uygulamalarında")]
    {
        h.set_locale(locale);
        let screen = h.screen();
        assert!(screen.contains(heading), "{locale}:\n{screen}");
        // One box under each of the three shared rows, none under motion or the pillar.
        assert_eq!(screen.matches(everywhere).count(), 3, "{locale}:\n{screen}");
        assert!(!screen.contains('⟦'), "no key is missing in {locale}:\n{screen}");
    }
}

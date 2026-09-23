//! The notice of a newer qpac, driven from what the person touches: the switch on the settings
//! page, and the notice itself.
//!
//! No test reaches the network: the harness records the question instead of asking it and answers
//! with the version a test names. The family's folder and qpac's state folder are the scratch
//! machine's own, so the person's own switch is never read or turned off.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::storage::Family;

use super::SelfUpdateFolders;
use crate::app::Qpackages;
use crate::locales::tests::{PENDING, codes, label};
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in, first_start, programs};

/// A version no release of qpac will reach while these tests are read.
const NEWER: &str = "9.9.9";

fn machine(name: &str) -> Scratch {
    Scratch::new(name, &[Sample::new("bash", "5.3-1", "Shell")])
}

/// The family's folder and qpac's state folder of `scratch`. The family folder is the one the
/// appearance rows of [`app_in`] save into, so the switch and those rows share one file, as they
/// do on a real machine.
fn folders(scratch: &Scratch) -> SelfUpdateFolders {
    SelfUpdateFolders { config: scratch.root().to_path_buf(), state: scratch.root().join("state") }
}

/// Lets what was started run: the first read, a save.
fn settle(h: &mut Harness<Qpackages>) {
    for _ in 0..6 {
        h.advance(Duration::from_millis(20));
    }
}

/// qpac started on `scratch` after its setup, in English, `width` columns wide.
fn started(scratch: &Scratch, width: u16) -> Harness<Qpackages> {
    let recorded = Arc::new(Recorded::default());
    let settings = qframe::storage::Settings::open(scratch.root().join("packages.conf"));
    let app = app_in(scratch, settings, &recorded).with_self_update(Some(folders(scratch)));
    let mut h = Harness::with_env(app, crate::locales::env(), width, 120);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    settle(&mut h);
    h
}

/// Opens the settings with the button in the header, as the person does.
fn open_settings(h: &mut Harness<Qpackages>) {
    let glyph = h.env().icons().glyph("settings").into_owned();
    let (x, y) = h.find(&glyph).unwrap_or_else(|| panic!("the settings button:\n{}", h.screen()));
    h.click(x, y);
    settle(h);
    assert!(h.app().settings_open(), "{}", h.screen());
}

/// Clicks the switch of the row whose name is `row`. A switch is colour alone, so it is found by
/// where it stands: at the right edge of the list's controls, which a drop-down's arrow marks.
fn click_switch(h: &mut Harness<Qpackages>, row: &str) {
    let (_, y) = h.find(row).unwrap_or_else(|| panic!("`{row}`:\n{}", h.screen()));
    let (edge, _) = h.find("▾").unwrap_or_else(|| panic!("a drop-down:\n{}", h.screen()));
    h.click(edge - 1, y);
    settle(h);
}

/// Where the question would remember itself, had it really been asked.
fn remembered(scratch: &Scratch) -> PathBuf {
    folders(scratch).state.join("update-check")
}

#[test]
fn a_newer_qpac_is_said_as_qpac_not_as_the_packages_and_the_same_one_is_not() {
    let scratch = machine("self-update-newer");
    let mut h = started(&scratch, 120);
    let asked = h.update_checks().to_vec();
    assert_eq!(asked.len(), 1, "qpac asks once at start");
    assert_eq!((asked[0].package(), asked[0].current()), ("quvyta-packages", env!("CARGO_PKG_VERSION")));
    assert!(!h.screen().contains("is out"), "nothing is said before the answer:\n{}", h.screen());

    h.set_latest_version(Some(NEWER));
    settle(&mut h);
    let screen = h.screen();
    assert!(screen.contains(&format!("qpac {NEWER} is out")), "the notice names qpac and the version:\n{screen}");
    assert!(screen.contains("not your packages"), "and says it is not about the packages:\n{screen}");
    assert!(screen.contains(env!("CARGO_PKG_VERSION")), "and the version running:\n{screen}");
    assert!(!remembered(&scratch).exists(), "the harness asked nothing for real");

    for latest in [env!("CARGO_PKG_VERSION"), "0.0.1"] {
        let scratch = machine("self-update-same");
        let mut same = started(&scratch, 120);
        same.set_latest_version(Some(latest));
        settle(&mut same);
        assert!(!same.screen().contains("is out"), "{latest} is not newer:\n{}", same.screen());
    }
}

#[test]
fn nothing_is_asked_while_the_setup_wizard_is_open() {
    let scratch = machine("self-update-wizard");
    let recorded = Arc::new(Recorded::default());
    let family = SelfUpdateFolders { config: scratch.config(), state: scratch.root().join("state") };
    let app = first_start(&scratch, &recorded, programs, 1000).with_self_update(Some(family));
    let mut h = Harness::with_env(app, crate::locales::env(), 100, 40);
    settle(&mut h);
    assert!(h.app().setting_up(), "the wizard is open:\n{}", h.screen());
    assert!(h.update_checks().is_empty(), "nothing is asked while the wizard is open");
}

#[test]
fn the_switch_on_the_settings_page_turns_the_question_off_for_the_family_and_back_on() {
    let scratch = machine("self-update-switch");
    let mut h = started(&scratch, 120);
    assert_eq!(h.update_checks().len(), 1, "on until someone turns it off");
    open_settings(&mut h);
    click_switch(&mut h, "Say when a newer qpac is out");
    assert!(!Family::QUVYTA.update_notice_in(scratch.root()), "off:\n{}", h.screen());
    let shared = fs::read_to_string(scratch.root().join("quvyta.conf")).expect("the family's file");
    assert!(shared.contains("update-notice = false"), "{shared}");

    let mut off = started(&scratch, 120);
    assert!(off.update_checks().is_empty(), "a qpac started with it off asks nothing at all");
    off.set_latest_version(Some(NEWER));
    settle(&mut off);
    assert!(!off.screen().contains("is out"), "{}", off.screen());

    open_settings(&mut off);
    click_switch(&mut off, "Say when a newer qpac is out");
    assert!(Family::QUVYTA.update_notice_in(scratch.root()), "back on:\n{}", off.screen());
    let on = started(&scratch, 120);
    assert_eq!(on.update_checks().len(), 1, "and the next start asks again");
}

#[test]
fn the_switch_turned_off_elsewhere_in_the_family_is_off_here_too() {
    let scratch = machine("self-update-elsewhere");
    Family::QUVYTA.set_update_notice_in(scratch.root(), false).expect("written");
    let mut h = started(&scratch, 120);
    assert!(h.update_checks().is_empty(), "the family's file says off");
    open_settings(&mut h);
    // The page shows it off: turning it on is one click, and asks nothing until the next start.
    click_switch(&mut h, "Say when a newer qpac is out");
    assert!(Family::QUVYTA.update_notice_in(scratch.root()), "{}", h.screen());
    assert!(h.update_checks().is_empty(), "switching asks nothing by itself");
}

#[test]
fn the_package_updates_and_qpac_itself_stand_under_headings_of_their_own() {
    let scratch = machine("self-update-apart");
    let mut h = started(&scratch, 120);
    open_settings(&mut h);
    let screen = h.screen();
    let at = |text: &str| screen.find(text).unwrap_or_else(|| panic!("`{text}`:\n{screen}"));
    // The package check is under Updates, near the top; qpac itself is the last heading.
    assert!(at("Check in the background") < at("Appearance"), "{screen}");
    assert!(at("Appearance") < at("qpac itself"), "{screen}");
    assert!(at("qpac itself") < at("Say when a newer qpac is out"), "{screen}");
    assert!(!screen.contains("Say when an update is out"), "the family's words, ambiguous here:\n{screen}");

    let scratch = machine("self-update-none");
    let recorded = Arc::new(Recorded::default());
    let mut without = Harness::with_env(
        app_in(&scratch, qframe::storage::Settings::in_memory(), &recorded),
        crate::locales::env(),
        120,
        120,
    );
    without.set_locale("en");
    settle(&mut without);
    open_settings(&mut without);
    assert!(!without.screen().contains("qpac itself"), "no row where nothing is asked:\n{}", without.screen());
}

#[test]
fn the_heading_and_the_switch_stand_whole_in_every_language_on_a_narrow_screen() {
    for code in codes() {
        for width in [36, 60, 80, 120] {
            let scratch = machine("self-update-labels");
            let mut h = started(&scratch, width);
            h.set_locale(&code);
            open_settings(&mut h);
            let screen = h.screen();
            for key in ["self-update.heading", "self-update.switch"] {
                // A language still waiting for the key shows English, as the framework's lookup does.
                let shown = if PENDING.contains(&key) && !["en", "tr"].contains(&code.as_str()) { "en" } else { &code };
                let text = label(shown, key);
                assert!(screen.contains(&text), "`{key}` is cut in `{code}` at {width}; it reads `{text}`:\n{screen}");
            }
            for line in screen.lines() {
                assert!(qframe::text::width(line) <= width, "`{line}` is too wide in `{code}` at {width}");
            }
            assert!(!screen.contains('⟦'), "a key is missing in `{code}`:\n{screen}");
        }
    }
}

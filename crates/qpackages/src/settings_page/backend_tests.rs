//! The settings around the packages on a pretend machine: the background check's timer written
//! into a scratch unit folder, the snapshot choice among the tools the scratch root has, the
//! orphan choice, and reflector's rows, whose privileged work goes through the recorded helper.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::runtime::ProcessOutcome;
use qframe::storage::Settings;
use qpackages_core::helper::SYSTEMCTL_PATH;
use qpackages_core::pacman::command::{PACMAN, print_install};
use qpackages_core::reflector::{Country, Mirrors, Protocol, REFLECTOR_PATH, Sort, list_countries_args};

use super::{BackendMsg, Msg};
use crate::app::{Msg as AppMsg, Qpackages};
use crate::autostart::{self, SYSTEMCTL, TIMER};
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_with, click_last, programs};

/// What `reflector --list-countries` prints: a heading, a rule, then name, code and count.
const COUNTRIES: &str = "Country                Code Count\n\
                         ---------------------- ---- -----\n\
                         Germany                DE     350\n\
                         Turkey                 TR       7\n\
                         United States          US     400\n";

/// The pretend machine's programs, and reflector.
fn with_reflector(program: &str) -> Option<PathBuf> {
    programs(program).or_else(|| (program == "reflector").then(|| Path::new("/usr/bin").join(program)))
}

/// The application with its settings page open on a machine whose programs `lookup` finds and
/// whose root holds `files`; the settings file is in the scratch folder.
fn page(files: &[&str], lookup: fn(&str) -> Option<PathBuf>) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("backend-settings", &[Sample::new("bash", "5.3-1", "Shell")]);
    for file in files {
        let path = scratch.root().join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
        fs::write(path, "").expect("a file");
    }
    let recorded = Arc::new(Recorded::default());
    recorded.answer("reflector", &list_countries_args(), COUNTRIES, 0);
    let settings = Settings::open(scratch.root().join("packages.conf"));
    let mut h = Harness::with_env(app_with(&scratch, settings, &recorded, lookup), crate::locales::env(), 110, 90);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.advance(Duration::from_millis(20));
    h.send(AppMsg::OpenSettings);
    h.advance(Duration::from_millis(20));
    (h, scratch, recorded)
}

fn send(h: &mut Harness<Qpackages>, msg: BackendMsg) {
    h.send(AppMsg::Settings(Msg::Backend(msg)));
    h.advance(Duration::from_millis(20));
}

/// What the settings file says after the background save ran.
fn written(scratch: &Scratch) -> String {
    fs::read_to_string(scratch.root().join("packages.conf")).unwrap_or_default()
}

#[test]
fn the_background_check_writes_its_timer_and_follows_the_interval() {
    let (mut h, scratch, recorded) = page(&[], programs);
    recorded.answer(SYSTEMCTL, &autostart::reload_args(), "", 0);
    recorded.answer(SYSTEMCTL, &autostart::enable_args(), "", 0);
    recorded.answer(SYSTEMCTL, &autostart::disable_args(), "", 0);
    send(&mut h, BackendMsg::Autostart(true));
    let timer = scratch.units().join(TIMER);
    assert!(fs::read_to_string(&timer).expect("the timer is written").contains("OnUnitActiveSec=6h"));
    assert!(written(&scratch).contains("autostart = \"on\""), "{}", written(&scratch));
    let lines = recorded.command_lines();
    assert!(lines.contains(&"systemctl --user enable --now -- quvyta-packages-check.timer".to_owned()), "{lines:?}");
    send(&mut h, BackendMsg::Interval(4));
    assert!(fs::read_to_string(&timer).expect("written again").contains("OnUnitActiveSec=24h"));
    assert!(written(&scratch).contains("interval = 24"));
    assert!(h.screen().contains("day"), "{}", h.screen());
    send(&mut h, BackendMsg::Autostart(false));
    assert!(!timer.exists(), "switching off removes the units");
    assert!(written(&scratch).contains("autostart = \"off\""));
}

#[test]
fn a_timer_that_cannot_be_switched_puts_the_choice_back_and_says_why() {
    let (mut h, scratch, recorded) = page(&[], programs);
    recorded.answer(SYSTEMCTL, &autostart::reload_args(), "", 0);
    recorded.fail(SYSTEMCTL, &autostart::enable_args(), "Failed to connect to bus", 1);
    send(&mut h, BackendMsg::Autostart(true));
    h.advance(Duration::from_millis(50));
    let screen = h.screen();
    assert!(screen.contains("The background check could not be turned on"), "{screen}");
    assert!(written(&scratch).contains("autostart = \"off\""), "{}", written(&scratch));
}

#[test]
fn only_an_installed_snapshot_tool_can_be_chosen() {
    let (mut h, scratch, _) = page(&["usr/bin/snapper", "etc/snapper/configs/root"], programs);
    let screen = h.screen();
    assert!(screen.contains("A snapper snapshot is taken before and after updating."), "{screen}");
    let timeshift = screen.lines().find(|line| line.trim_start().starts_with("timeshift")).expect("its row");
    assert!(timeshift.contains("not installed"), "{screen}");
    send(&mut h, BackendMsg::Backup(0));
    assert!(written(&scratch).contains("tool = \"off\""), "{}", written(&scratch));
    assert!(h.screen().contains("No snapshot is taken."), "{}", h.screen());
    send(&mut h, BackendMsg::Backup(5));
    assert!(written(&scratch).contains("tool = \"off\""), "a choice that is not offered changes nothing");
}

#[test]
fn snap_pac_is_named_where_it_takes_the_snapshots() {
    let files = ["usr/bin/snapper", "etc/snapper/configs/root", "var/lib/pacman/local/snap-pac-3.0.1-1/desc"];
    let (h, _scratch, _) = page(&files, programs);
    assert!(h.screen().contains("snap-pac takes snapper snapshots around every update."), "{}", h.screen());
}

#[test]
fn the_orphan_choice_is_saved_at_once() {
    let (mut h, scratch, _) = page(&[], programs);
    assert!(h.screen().contains("Ask after"), "{}", h.screen());
    send(&mut h, BackendMsg::Orphans(2));
    assert!(written(&scratch).contains("orphans = \"auto\""), "{}", written(&scratch));
    assert!(h.screen().contains("Remove them"), "{}", h.screen());
}

#[test]
fn without_reflector_its_row_offers_to_install_it() {
    let (mut h, _scratch, recorded) = page(&[], programs);
    let screen = h.screen();
    assert!(screen.contains("Install reflector"), "{screen}");
    assert!(!screen.contains("Countries"), "{screen}");
    assert!(!recorded.command_lines().iter().any(|line| line.starts_with("reflector")), "not asked when missing");
    recorded.answer(PACMAN, &print_install(&["reflector"]), "extra|reflector|2023-5|40000\n", 0);
    send(&mut h, BackendMsg::InstallReflector);
    assert!(h.screen().contains("Install 1 package?"), "{}", h.screen());
}

#[test]
fn countries_are_read_without_privileges_chosen_and_saved() {
    let (mut h, scratch, recorded) = page(&[], with_reflector);
    let screen = h.screen();
    assert!(screen.contains("every country"), "{screen}");
    assert!(recorded.command_lines().contains(&"reflector --list-countries".to_owned()));
    send(&mut h, BackendMsg::ChooseCountries);
    let screen = h.screen();
    for text in ["Mirrors are taken from the countries checked", "Germany", "Turkey", "United States", "400"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    send(&mut h, BackendMsg::ToggleCountry(1));
    send(&mut h, BackendMsg::ToggleCountry(0));
    assert!(written(&scratch).contains("countries = [\"TR\", \"DE\"]"), "{}", written(&scratch));
    send(&mut h, BackendMsg::CountriesDone);
    assert!(h.screen().contains("Turkey, Germany"), "{}", h.screen());
    send(&mut h, BackendMsg::MirrorCount(0));
    send(&mut h, BackendMsg::MirrorAge(2));
    send(&mut h, BackendMsg::MirrorSort(1));
    let text = written(&scratch);
    assert!(text.contains("count = 5") && text.contains("age = 24") && text.contains("sort = \"age\""), "{text}");
}

#[test]
fn applying_the_mirrors_confirms_and_sends_the_checked_values_to_the_helper() {
    let (mut h, scratch, recorded) = page(&[], with_reflector);
    // The helper of the tests works under the scratch folder, where reflector's list would be
    // written; nothing of the real machine's `/etc` is ever touched.
    let fresh = scratch.root().join("etc/pacman.d/.mirrorlist.qpac-new");
    let known = [Country { name: "Turkey".to_owned(), code: "TR".to_owned(), mirrors: 7 }];
    let mirrors = Mirrors::new(&["TR"], &known, Protocol::Https, 12, 10, Sort::Rate).expect("valid");
    recorded.play(REFLECTOR_PATH, &mirrors.args(&fresh), &["done"], ProcessOutcome::Finished { code: Some(0) });
    send(&mut h, BackendMsg::ToggleCountry(1));
    send(&mut h, BackendMsg::ApplyMirrors);
    let screen = h.screen();
    for text in ["Choose new mirrors?", "Countries: TR", "The 10 most recently synchronized", "download speed"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    click_last(&mut h, "Choose mirrors");
    for _ in 0..4 {
        h.advance(Duration::from_millis(20));
    }
    let run =
        recorded.command_lines().into_iter().find(|line| line.starts_with(REFLECTOR_PATH)).expect("reflector ran");
    assert!(run.ends_with("--protocol https --age 12 --latest 10 --sort rate --country TR"), "{run}");
    // The recording writes no list, so the helper keeps the old one and says so.
    assert!(h.screen().contains("reflector found no mirror"), "{}", h.screen());
}

#[test]
fn reflector_s_timer_is_switched_through_the_helper_after_a_confirmation() {
    let (mut h, _scratch, recorded) = page(&[], with_reflector);
    let enable = ["enable", "--now", "--", "reflector.timer"];
    recorded.play(SYSTEMCTL_PATH, &enable, &[], ProcessOutcome::Finished { code: Some(0) });
    send(&mut h, BackendMsg::ReflectorTimer(true));
    assert!(h.screen().contains("Turn on reflector's weekly timer?"), "{}", h.screen());
    click_last(&mut h, "Turn on");
    for _ in 0..4 {
        h.advance(Duration::from_millis(20));
    }
    assert!(recorded.command_lines().contains(&format!("{SYSTEMCTL_PATH} enable --now -- reflector.timer")));
    assert!(h.screen().contains("reflector's timer is on"), "{}", h.screen());
}

#[test]
fn the_new_rows_keep_the_rules_in_every_glyph_mode_narrow_and_in_turkish() {
    for width in [44, 110] {
        let (mut h, _scratch, _) = page(&["usr/bin/timeshift"], with_reflector);
        h.resize(width, 90);
        for mode in [GlyphMode::Unicode, GlyphMode::Ascii, GlyphMode::Nerd] {
            h.set_glyph_mode(mode);
            let screen = h.screen();
            for text in ["Updates", "Backup", "Cleanup", "Mirrors"] {
                assert!(screen.contains(text), "`{text}` at {width} in {mode:?}:\n{screen}");
            }
            for forbidden in ['[', ']', '{', '}', '|'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` at {width} in {mode:?}:\n{screen}");
            }
        }
        h.set_locale("tr");
        let screen = h.screen();
        for text in ["Güncellemeler", "Yedek", "Temizlik", "Aynalar"] {
            assert!(screen.contains(text), "`{text}` at {width}:\n{screen}");
        }
        assert!(!screen.contains('⟦'), "a key is missing in Turkish:\n{screen}");
        send(&mut h, BackendMsg::ChooseCountries);
        assert!(h.screen().contains("Türkiye") || h.screen().contains("Turkey"), "{}", h.screen());
    }
}

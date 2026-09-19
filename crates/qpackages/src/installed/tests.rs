//! The Installed tab on a pretend machine: two applications, an AUR helper built from the AUR and
//! two libraries pulled in as dependencies.

use std::sync::Arc;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::storage::Settings;
use qpackages_core::pacman::command::{PACMAN, foreign, print_remove};

use crate::app::{Msg, Qpackages};
use crate::installed::Msg as Tab;
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in};

const SAMPLES: [Sample; 5] = [
    Sample::new("firefox", "143.0.1-1", "Fast, Private & Safe Web Browser").app(),
    Sample::new("gimp", "3.0.4-1", "GNU Image Manipulation Program").app(),
    Sample::new("paru", "2.1.0-1", "Feature packed AUR helper"),
    Sample::new("glibc", "2.42-2", "GNU C Library").dependency(),
    Sample::new("zlib", "1.3.1-2", "Compression library").dependency(),
];

/// The screen over the pretend machine, where pacman says paru came from no repository.
fn screen(width: u16, height: u16) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("installed", &SAMPLES);
    let recorded = Arc::new(Recorded::default());
    recorded.answer(PACMAN, &foreign(), "paru\n", 0);
    let mut h = Harness::with_env(app_in(&scratch, Settings::in_memory(), &recorded), crate::test_env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    (h, scratch, recorded)
}

/// The row `name` stands on among every package, in name order.
fn row(name: &str) -> usize {
    let mut names: Vec<&str> = SAMPLES.iter().map(|sample| sample.name).collect();
    names.sort_unstable();
    names.iter().position(|candidate| *candidate == name).expect("a sample")
}

#[test]
fn it_opens_on_the_applications_with_their_source_and_the_counts() {
    let (h, _scratch, _) = screen(120, 24);
    let screen = h.screen();
    for text in ["firefox", "gimp", "143.0.1-1", "Source", "Repo", "Apps", "All packages", "2 apps · 5 packages"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    for text in ["glibc", "paru", "zlib"] {
        assert!(!screen.contains(text), "`{text}` is not an application:\n{screen}");
    }
}

#[test]
fn all_packages_lists_everything_and_names_the_aur_ones() {
    let (mut h, _scratch, _) = screen(120, 24);
    h.click_text("All packages");
    let screen = h.screen();
    for text in ["glibc", "zlib", "paru", "AUR", "5 packages"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let paru = screen.lines().find(|line| line.contains("paru")).expect("paru's row");
    assert!(paru.contains("AUR"), "{paru}");
    let glibc = screen.lines().find(|line| line.contains("glibc")).expect("glibc's row");
    assert!(glibc.contains("Repo"), "{glibc}");
    h.click_text("Apps");
    assert!(!h.screen().contains("glibc"), "{}", h.screen());
}

#[test]
fn the_search_reads_words_and_a_source_filter() {
    let (mut h, _scratch, _) = screen(120, 24);
    h.send(Msg::Installed(Tab::Show(1)));
    h.press("/");
    h.type_text("source:aur");
    let screen = h.screen();
    assert!(screen.contains("paru") && !screen.contains("glibc"), "{screen}");
    assert!(screen.contains("1 match · 5 packages"), "{screen}");
    h.send(Msg::Installed(Tab::Search("kaynak:depo library".to_owned())));
    let screen = h.screen();
    assert!(screen.contains("glibc") && screen.contains("zlib") && !screen.contains("paru"), "{screen}");
    h.send(Msg::Installed(Tab::Search("source:snap".to_owned())));
    assert!(h.screen().contains("No package matches the search"), "{}", h.screen());
}

#[test]
fn without_pacman_s_answer_no_package_claims_a_source() {
    let scratch = Scratch::new("installed-unknown", &SAMPLES);
    let recorded = Arc::new(Recorded::default());
    let mut h = Harness::with_env(app_in(&scratch, Settings::in_memory(), &recorded), crate::test_env(), 120, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    h.send(Msg::Installed(Tab::Show(1)));
    let screen = h.screen();
    assert!(!screen.contains("Repo") && !screen.contains("AUR "), "{screen}");
    h.send(Msg::Installed(Tab::Search("source:aur".to_owned())));
    assert!(h.screen().contains("No package matches the search"), "{}", h.screen());
}

#[test]
fn sorting_by_source_puts_the_aur_last() {
    let (mut h, _scratch, _) = screen(120, 24);
    h.send(Msg::Installed(Tab::Show(1)));
    h.click_text("Source");
    let screen = h.screen();
    let order: Vec<&str> =
        ["firefox", "gimp", "glibc", "zlib", "paru"].into_iter().filter(|name| screen.contains(name)).collect();
    let positions: Vec<usize> = order.iter().map(|name| screen.find(name).expect("on screen")).collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "the repositories first, by name:\n{screen}");
}

#[test]
fn the_details_stand_beside_the_table_on_a_wide_screen() {
    let (mut h, _scratch, _) = screen(120, 24);
    h.click_text("gimp");
    let screen = h.screen();
    assert!(screen.contains("GNU Image Manipulation Program"), "{screen}");
    assert!(screen.contains("firefox"), "the table stays:\n{screen}");
    assert!(!screen.contains("Back"), "{screen}");
}

#[test]
fn a_narrow_screen_opens_the_details_in_place_of_the_table_and_goes_back() {
    let (mut h, _scratch, _) = screen(80, 24);
    assert!(!h.screen().contains("Nothing selected"), "no empty detail panel:\n{}", h.screen());
    h.click_text("gimp");
    let screen = h.screen();
    assert!(screen.contains("GNU Image Manipulation Program") && screen.contains("Back"), "{screen}");
    assert!(!screen.contains("firefox"), "the details take the tab:\n{screen}");
    h.press("esc");
    assert!(h.screen().contains("firefox"), "escape goes back:\n{}", h.screen());
    h.click_text("gimp");
    h.click_text("Back");
    assert!(h.screen().contains("firefox"), "{}", h.screen());
    h.send(Msg::Installed(Tab::Open(0)));
    assert!(h.screen().contains("Back"), "{}", h.screen());
    h.send(Msg::Installed(Tab::Search("fire".to_owned())));
    let screen = h.screen();
    assert!(screen.contains("firefox") && !screen.contains("Back"), "a new search shows the table again:\n{screen}");
}

#[test]
fn checks_survive_a_search_and_the_removal_starts_from_the_summary_line() {
    let (mut h, _scratch, recorded) = screen(120, 24);
    recorded.answer(PACMAN, &print_remove(&["paru", "zlib"]), "paru|2.1.0-1\nzlib|1.3.1-2\n", 0);
    h.send(Msg::Installed(Tab::Show(1)));
    assert!(!h.screen().contains("Remove checked"), "nothing to remove yet:\n{}", h.screen());
    h.send(Msg::Installed(Tab::Toggle(row("paru")))).send(Msg::Installed(Tab::Toggle(row("zlib"))));
    h.send(Msg::Installed(Tab::Search("nothing".to_owned())));
    let screen = h.screen();
    assert!(screen.contains("2 packages checked") && screen.contains("Remove checked"), "{screen}");
    h.send(Msg::Installed(Tab::Search(String::new())));
    h.click_text("Remove checked");
    let screen = h.screen();
    assert!(screen.contains("Remove 2 packages?") && screen.contains("zlib  1.3.1-2"), "{screen}");
    h.press("esc");
    h.press("delete");
    assert!(h.screen().contains("Remove 2 packages?"), "the key asks the same:\n{}", h.screen());
}

#[test]
fn a_machine_without_launchers_says_where_the_packages_are() {
    let scratch = Scratch::new("installed-plain", &[Sample::new("bash", "5.3-1", "Shell")]);
    let recorded = Arc::new(Recorded::default());
    let mut h = Harness::with_env(app_in(&scratch, Settings::in_memory(), &recorded), crate::test_env(), 120, 24);
    h.set_locale("en");
    assert!(h.screen().contains("No installed application has a launcher"), "{}", h.screen());
    h.set_locale("tr");
    assert!(h.screen().contains("hiçbirinin başlatıcısı yok"), "{}", h.screen());
}

#[test]
fn every_glyph_mode_and_width_keeps_the_rules() {
    for (width, height) in [(36, 14), (60, 20), (80, 24), (120, 30)] {
        let (mut h, _scratch, _) = screen(width, height);
        h.send(Msg::Installed(Tab::Show(1)));
        for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
            h.set_glyph_mode(mode);
            let screen = h.screen();
            assert!(screen.contains("glibc"), "{width}x{height} {mode:?}:\n{screen}");
            assert!(!screen.contains('⟦'), "{screen}");
            if mode == GlyphMode::Ascii {
                for forbidden in ['[', ']', '{', '}', '|'] {
                    assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
                }
            }
        }
        let screen = h.screen();
        if width < 40 {
            assert!(!screen.contains("AUR") && !screen.contains("Repo"), "no source below 40 columns:\n{screen}");
        } else {
            assert!(screen.contains("AUR"), "{width}x{height}:\n{screen}");
        }
        if width < 60 {
            let search = screen.lines().position(|line| line.contains("Search")).expect("the search line");
            let choice = screen.lines().position(|line| line.contains("All packages")).expect("the choice");
            assert!(choice > search, "the choice goes under the search when narrow:\n{screen}");
        }
    }
}

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
    let mut h = Harness::with_env(app_in(&scratch, settings, &recorded), crate::locales::env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.send(AppMsg::OpenSettings);
    (h, scratch, recorded)
}

/// What the ecosystem's shared file says.
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
    let (mut h, scratch, _) = page(100, 48);
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
    // Walking down far enough reaches the last section, which no screen this short can show at
    // once.
    for _ in 0..40 {
        h.press("down");
    }
    let screen = h.screen();
    assert!(screen.contains("Pillar"), "the keys reached the last row:\n{screen}");
    assert!(!screen.contains("Sources"), "and the list scrolled to it:\n{screen}");
}

#[test]
fn the_appearance_rows_read_in_both_languages_and_offer_every_app() {
    let (mut h, _scratch, _) = page(100, 80);
    for (locale, heading, everywhere) in
        [("en", "Appearance", "In every Quvyta application"), ("tr", "Görünüm", "Tüm Quvyta uygulamalarında")]
    {
        h.set_locale(locale);
        let screen = h.screen();
        assert!(screen.contains(heading), "{locale}:\n{screen}");
        // One box under each of the four shared rows, reduced motion among them; none under the pillar.
        assert_eq!(screen.matches(everywhere).count(), 4, "{locale}:\n{screen}");
        assert!(!screen.contains('⟦'), "no key is missing in {locale}:\n{screen}");
    }
}

/// Walks to the row labelled `label` the way a person does with the keyboard alone: tab until the
/// settings list has the focus, then down until the row is the selected one, which the pillar in
/// front of it shows.
fn keys_to(h: &mut Harness<Qpackages>, label: &str) {
    let selected = |h: &Harness<Qpackages>, text: &str| h.screen().lines().any(|line| line.starts_with(text));
    for _ in 0..12 {
        if selected(h, "▌  ") {
            break;
        }
        h.press("tab");
    }
    assert!(selected(h, "▌  "), "tab never reached the settings:\n{}", h.screen());
    let row = format!("▌  {label}");
    for _ in 0..60 {
        if selected(h, &row) {
            return;
        }
        h.press("down");
    }
    panic!("down never reached `{label}`:\n{}", h.screen());
}

/// The line the row labelled `label` is drawn on.
fn line_of(h: &Harness<Qpackages>, label: &str) -> String {
    let screen = h.screen();
    screen
        .lines()
        .find(|line| line.trim_start_matches(['▌', '▎', '▏', ' ']).starts_with(label))
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn the_icons_row_is_turned_with_the_keys_and_the_screen_redraws_in_its_glyphs() {
    let (mut h, scratch, _) = page(100, 80);
    assert!(h.screen().contains('▾'), "unicode chevrons first:\n{}", h.screen());
    keys_to(&mut h, "Icons");
    // Enter opens the choice, End goes to its last entry, ASCII, and Enter takes it.
    h.press("enter").press("end").press("enter");
    let screen = h.screen();
    assert!(!screen.contains('▾'), "no unicode glyph is left:\n{screen}");
    assert!(line_of(&h, "Icons").contains("ASCII"), "the row names its mode:\n{screen}");
    // The row opens following the shared settings, so the value goes to the shared file and qpac's
    // own file says that it follows.
    assert!(shared_file(&scratch).contains("icons = \"ascii\""), "{}", shared_file(&scratch));
    assert!(written(&mut h, &scratch).contains("icons = \"quvyta\""), "{}", written(&mut h, &scratch));
}

#[test]
fn the_language_row_is_turned_with_the_keys_and_the_page_speaks_it_at_once() {
    let (mut h, scratch, _) = page(100, 80);
    keys_to(&mut h, "Language");
    // Typing the first letter of a name finds it in the open list.
    h.press("enter").press("t").press("enter");
    let screen = h.screen();
    for text in ["Ayarlar", "Görünüm", "Kaynaklar"] {
        assert!(screen.contains(text), "`{text}`:\n{screen}");
    }
    assert!(!screen.contains("Appearance"), "{screen}");
    assert!(shared_file(&scratch).contains("language = \"tr\""), "{}", shared_file(&scratch));
    assert!(written(&mut h, &scratch).contains("language = \"quvyta\""), "{}", written(&mut h, &scratch));
}

#[test]
fn the_theme_row_is_turned_with_the_keys_and_the_screen_takes_its_colours() {
    let (mut h, scratch, _) = page(100, 80);
    let before = h.env().theme().id().to_owned();
    let colours = |h: &Harness<Qpackages>| -> Vec<_> { (0..100).map(|x| (h.fg(x, 2), h.bg(x, 2))).collect() };
    let drawn = colours(&h);
    keys_to(&mut h, "Theme");
    h.press("enter").press("down").press("enter");
    let after = h.env().theme().id().to_owned();
    assert_ne!(after, before, "{}", h.screen());
    assert_ne!(colours(&h), drawn, "the page is drawn in the new theme");
    assert!(shared_file(&scratch).contains(&format!("theme = \"{after}\"")), "{}", shared_file(&scratch));
}

#[test]
fn the_box_under_a_row_is_cleared_with_the_keys_and_the_next_choice_stays_with_qpac() {
    let (mut h, scratch, _) = page(100, 80);
    keys_to(&mut h, "Icons");
    h.press("down").press("space");
    let before = shared_file(&scratch);
    // Back on the row, Home and one down is the second mode, Nerd Font.
    h.press("up").press("enter").press("home").press("down").press("enter");
    let own = written(&mut h, &scratch);
    assert!(own.contains("icons = \"nerd\""), "{own}");
    assert_eq!(shared_file(&scratch), before, "the shared file is left alone");
    assert!(!h.screen().contains('▾'), "the nerd glyphs are drawn:\n{}", h.screen());
}

#[test]
fn the_pillar_is_moved_with_the_keys_and_the_selected_row_wears_it() {
    let (mut h, scratch, _) = page(100, 80);
    keys_to(&mut h, "Pillar");
    h.press("right");
    let screen = h.screen();
    assert!(!screen.lines().any(|line| line.starts_with('▌')), "the thick pillar is gone:\n{screen}");
    assert!(screen.lines().any(|line| line.starts_with("▎  Pillar")), "the row wears the thin one:\n{screen}");
    assert!(written(&mut h, &scratch).contains("pillar = \"thin\""), "{}", written(&mut h, &scratch));
}

#[test]
fn reduced_motion_is_switched_with_the_keys() {
    let (mut h, scratch, _) = page(100, 80);
    let before = h.env().reduced_motion();
    keys_to(&mut h, "Reduce motion");
    h.press("space");
    assert_ne!(h.env().reduced_motion(), before, "{}", h.screen());
    let shared = shared_file(&scratch);
    assert!(shared.contains(&format!("reduced-motion = {}", !before)), "{shared}");
}

#[test]
fn a_theme_another_application_gives_qpac_while_it_is_open_is_where_the_next_pick_goes() {
    use qframe::storage::{Ecosystem, Scope, Shared};
    let scratch = Scratch::new("follow", &[Sample::new("bash", "5.3-1", "Shell")]);
    let folder = scratch.root().to_path_buf();
    fs::write(folder.join("quvyta.conf"), "language = \"en\"\ntheme = \"monochrome\"\nicons = \"unicode\"\n")
        .expect("the shared file");
    let recorded = Arc::new(Recorded::default());
    let app = app_in(&scratch, Settings::open(folder.join("packages.conf")), &recorded);
    let mut h = Harness::member_in(app, Ecosystem::QUVYTA, &folder, settings::APP, 100, 80);
    h.set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.send(AppMsg::OpenSettings);
    // Another member, the launcher say, gives qpac a theme of its own in qpac's file.
    Ecosystem::QUVYTA.set_in(&folder, settings::APP, Shared::Theme, "iris", Scope::App).expect("saved");
    h.poll_preferences();
    assert_eq!(h.env().theme().id(), "iris", "the screen follows");
    assert!(line_of(&h, "Theme").contains("Iris"), "and the row says so:\n{}", h.screen());
    // qpac keeps its own theme now, so the next one picked here stays with qpac.
    keys_to(&mut h, "Theme");
    h.press("enter").press("down").press("enter");
    let picked = h.env().theme().id().to_owned();
    assert_ne!(picked, "iris", "{}", h.screen());
    let own = written(&mut h, &scratch);
    assert!(own.contains(&format!("theme = \"{picked}\"")), "{own}");
    assert!(
        shared_file(&scratch).contains("theme = \"monochrome\""),
        "the others keep theirs:\n{}",
        shared_file(&scratch)
    );
}

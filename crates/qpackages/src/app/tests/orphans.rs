//! Orphans through the screen: marked among every package with a line that cleans them up, the
//! cleanup's confirmation and its request, a list that changed in the meantime, and what the
//! cleanup setting does after a removal. The helper plays pacman from the recording.

use std::sync::Arc;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::ProcessOutcome;
use qframe::storage::Settings;
use qpackages_core::helper::PACMAN_PATH;
use qpackages_core::pacman::command::{PACMAN, orphans, print_remove, remove};

use super::flow::{click_last, settle};
use crate::app::{Msg, Qpackages};
use crate::installed;
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in};

/// yay, asked for, and libfoo, pulled in by it once.
const SAMPLES: [Sample; 2] =
    [Sample::new("yay", "12.5.0-1", "AUR helper"), Sample::new("libfoo", "1.0-1", "A library").dependency()];

/// The application on a machine with the samples, where pacman lists `orphans` as the orphans,
/// following `settings`, at `width` × `height`.
fn screen(orphaned: &str, settings: &str, width: u16, height: u16) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("orphans", &SAMPLES);
    let recorded = Arc::new(Recorded::default());
    answer_orphans(&recorded, orphaned);
    recorded.answer(PACMAN, &print_remove(&["libfoo"]), "libfoo|1.0-1\n", 0);
    recorded.answer(PACMAN, &print_remove(&["yay"]), "yay|12.5.0-1\n", 0);
    let settings = Settings::parse_str("packages.conf", settings);
    let mut h = Harness::with_env(app_in(&scratch, settings, &recorded), crate::test_env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    settle(&mut h);
    (h, scratch, recorded)
}

/// Has pacman, asked by qpac or by the helper, list `names` as the orphans.
fn answer_orphans(recorded: &Recorded, names: &str) {
    let code = if names.is_empty() { 1 } else { 0 };
    recorded.answer(PACMAN, &orphans(), names, code);
    recorded.answer(PACMAN_PATH, &orphans(), names, code);
}

/// Every package, where the orphans are.
fn all_packages(h: &mut Harness<Qpackages>) {
    h.send(Msg::Installed(installed::Msg::Show(1)));
}

/// What the helper had pacman do, as `program arg arg…` lines.
fn pacman_runs(recorded: &Recorded) -> Vec<String> {
    recorded
        .command_lines()
        .into_iter()
        .filter(|line| line.starts_with(PACMAN_PATH) && !line.ends_with("-Qtdq"))
        .collect()
}

#[test]
fn orphans_are_faint_and_marked_among_every_package_with_a_line_that_cleans_them() {
    for (width, height) in [(50, 16), (120, 20)] {
        let (mut h, _scratch, _recorded) = screen("libfoo\n", "", width, height);
        let screen = h.screen();
        assert!(!screen.contains("orphan"), "not among the applications:\n{screen}");
        all_packages(&mut h);
        for mode in [GlyphMode::Unicode, GlyphMode::Ascii, GlyphMode::Nerd] {
            h.set_glyph_mode(mode);
            let screen = h.screen();
            let row = screen.lines().find(|line| line.contains("libfoo")).expect("the orphan's row");
            // A narrow table scrolls its last columns out of sight; the faint row and the line
            // below still tell.
            assert_eq!(row.contains("orphan"), width >= 100, "{mode:?} at {width}:\n{screen}");
            let yay = screen.lines().find(|line| line.contains("yay")).expect("yay's row");
            assert!(!yay.contains("orphan"), "{screen}");
            let last = screen.lines().rev().nth(1).expect("the summary line");
            assert!(last.contains("1 orphan") && last.contains("Clean up"), "{mode:?} at {width}:\n{screen}");
            for forbidden in ['[', ']', '{', '}', '|'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` at {width}:\n{screen}");
            }
        }
        h.set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("sahipsiz") && screen.contains("Temizle"), "{screen}");
    }
}

#[test]
fn a_machine_without_orphans_has_no_line_for_them() {
    let (mut h, _scratch, _recorded) = screen("", "", 120, 20);
    all_packages(&mut h);
    let screen = h.screen();
    assert!(!screen.contains("orphan") && !screen.contains("Clean up"), "{screen}");
}

#[test]
fn cleaning_up_confirms_the_list_and_the_helper_removes_exactly_it() {
    let (mut h, _scratch, recorded) = screen("libfoo\n", "", 120, 24);
    recorded.play(PACMAN_PATH, &remove(&["libfoo"]), &["removing libfoo"], ProcessOutcome::Finished { code: Some(0) });
    all_packages(&mut h);
    click_last(&mut h, "Clean up");
    let screen = h.screen();
    for text in ["Remove 1 orphaned package?", "Nothing needs these any more", "libfoo  1.0-1", "settings files"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    click_last(&mut h, "Remove");
    let screen = settle(&mut h);
    assert_eq!(pacman_runs(&recorded), [format!("{PACMAN_PATH} -Rns --noconfirm -- libfoo")]);
    assert!(screen.contains("Removed 1 orphaned package"), "{screen}");
}

#[test]
fn a_list_that_changed_meanwhile_removes_nothing_and_is_shown_again() {
    let (mut h, _scratch, recorded) = screen("libfoo\n", "", 120, 24);
    recorded.answer(PACMAN, &print_remove(&["libbar", "libfoo"]), "libbar|2.0-1\nlibfoo|1.0-1\n", 0);
    all_packages(&mut h);
    click_last(&mut h, "Clean up");
    // Another transaction orphaned libbar between the confirmation and the request.
    answer_orphans(&recorded, "libfoo\nlibbar\n");
    click_last(&mut h, "Remove");
    let screen = settle(&mut h);
    assert!(pacman_runs(&recorded).is_empty(), "nothing was removed: {:?}", recorded.command_lines());
    assert!(screen.contains("Remove 2 orphaned packages?"), "the fresh list is asked about:\n{screen}");
    assert!(screen.contains("libbar  2.0-1"), "{screen}");
}

#[test]
fn after_a_removal_the_orphans_left_are_offered_by_default() {
    let (mut h, _scratch, recorded) = screen("", "", 120, 24);
    recorded.play(PACMAN_PATH, &remove(&["yay"]), &["removing yay"], ProcessOutcome::Finished { code: Some(0) });
    h.send(Msg::Transaction(crate::transaction::Msg::Begin(crate::transaction::Action::Remove(vec![
        "yay".to_owned(),
    ]))));
    answer_orphans(&recorded, "libfoo\n");
    click_last(&mut h, "Remove");
    let screen = settle(&mut h);
    assert!(screen.contains("1 orphaned package remains"), "{screen}");
    assert_eq!(pacman_runs(&recorded), [format!("{PACMAN_PATH} -Rns --noconfirm -- yay")], "nothing more yet");
    click_last(&mut h, "Clean up");
    assert!(h.screen().contains("Remove 1 orphaned package?"), "{}", h.screen());
}

#[test]
fn the_notice_opens_every_package_where_the_orphans_are() {
    let (mut h, _scratch, _recorded) = screen("libfoo\n", "", 120, 24);
    h.send(Msg::ShowOrphans);
    let screen = h.screen();
    assert!(screen.lines().any(|line| line.contains("libfoo") && line.contains("orphan")), "{screen}");
}

#[test]
fn set_to_auto_the_orphans_go_at_the_end_without_asking() {
    let (mut h, _scratch, recorded) = screen("", "[cleanup]\norphans = \"auto\"\n", 120, 24);
    recorded.play(PACMAN_PATH, &remove(&["yay"]), &["removing yay"], ProcessOutcome::Finished { code: Some(0) });
    recorded.play(PACMAN_PATH, &remove(&["libfoo"]), &["removing libfoo"], ProcessOutcome::Finished { code: Some(0) });
    h.send(Msg::Transaction(crate::transaction::Msg::Begin(crate::transaction::Action::Remove(vec![
        "yay".to_owned(),
    ]))));
    answer_orphans(&recorded, "libfoo\n");
    click_last(&mut h, "Remove");
    settle(&mut h);
    let screen = settle(&mut h);
    assert_eq!(
        pacman_runs(&recorded),
        [format!("{PACMAN_PATH} -Rns --noconfirm -- yay"), format!("{PACMAN_PATH} -Rns --noconfirm -- libfoo")]
    );
    assert!(screen.contains("Removed 1 orphaned package"), "{screen}");
}

#[test]
fn set_to_never_nothing_is_said() {
    let (mut h, _scratch, recorded) = screen("", "[cleanup]\norphans = \"never\"\n", 120, 24);
    recorded.play(PACMAN_PATH, &remove(&["yay"]), &["removing yay"], ProcessOutcome::Finished { code: Some(0) });
    h.send(Msg::Transaction(crate::transaction::Msg::Begin(crate::transaction::Action::Remove(vec![
        "yay".to_owned(),
    ]))));
    answer_orphans(&recorded, "libfoo\n");
    click_last(&mut h, "Remove");
    let screen = settle(&mut h);
    assert!(!screen.contains("remains"), "{screen}");
    assert_eq!(pacman_runs(&recorded), [format!("{PACMAN_PATH} -Rns --noconfirm -- yay")]);
}

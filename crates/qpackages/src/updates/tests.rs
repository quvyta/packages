//! The Updates tab: the check run through the whole application on a pretend machine, and the
//! tab's states drawn on their own.

use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::storage::Settings;
use qpackages_core::pacman::command::{self, FAKEROOT, PACMAN};

use super::*;
use crate::app::{Msg as AppMsg, Qpackages};
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in};

fn update(name: &str, from: &str, to: &str) -> Update {
    Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
}

/// 14:02 UTC on a day in 2026.
const AT: i64 = 1_789_999_320;

/// Two repository updates, one of them the kernel, and one from the AUR.
fn found() -> Found {
    Found {
        at: AT,
        repo: Ok(vec![update("linux", "6.18.1-1", "6.18.2-1"), update("mesa", "25.2.3-1", "25.2.4-1")]),
        aur: Some(Ok(vec![update("visual-studio-code-bin", "1.104.0-1", "1.105.0-1")])),
    }
}

/// The tab alone, fed messages as the application would, so a running check can be held still.
struct Tab(Updates, Cx);

impl App for Tab {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        let _ = self.0.update(msg);
        Command::none()
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        self.0.view(ui, self.1);
    }
}

fn tab(width: u16, height: u16) -> Harness<Tab> {
    let cx = Cx { aur: true, utc_offset: 0, can_check: true };
    let mut h = Harness::with_env(Tab(Updates::default(), cx), crate::test_env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h
}

#[test]
fn updates_are_grouped_by_source_with_the_restart_note_on_the_kernel() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(found()));
    let screen = h.screen();
    let linux = format!("{:22}  {:9}  →  6.18.2-1", "linux", "6.18.1-1");
    for text in [
        "3 updates · last checked 14:02",
        "Check now",
        "Repositories",
        "AUR",
        linux.as_str(),
        "visual-studio-code-bin  1.104.0-1  →  1.105.0-1",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let linux = screen.lines().find(|line| line.contains("linux")).expect("the kernel's row");
    assert!(linux.contains("restart needed"), "{linux}");
    let mesa = screen.lines().find(|line| line.contains("mesa")).expect("mesa's row");
    assert!(!mesa.contains("restart"), "{mesa}");
    let repo = screen.find("Repositories").expect("a heading");
    assert!(repo < screen.find("AUR").expect("a heading"), "the repositories come first:\n{screen}");
    assert!(!screen.contains("Update all"), "installing the updates waits for the helper:\n{screen}");
}

#[test]
fn an_empty_group_is_left_out_and_a_held_back_package_is_not_counted() {
    let mut h = tab(100, 20);
    let mut held = update("linux", "6.18.1-1", "6.18.2-1");
    held.ignored = true;
    h.send(Msg::Checked(Found { at: AT, repo: Ok(vec![held]), aur: Some(Ok(Vec::new())) }));
    let screen = h.screen();
    assert!(screen.contains("0 updates"), "{screen}");
    assert!(screen.contains("held back by IgnorePkg"), "{screen}");
    assert!(!screen.contains("AUR"), "the AUR has nothing, so it has no heading:\n{screen}");
    assert_eq!(h.app().0.pending(true), 0);
}

#[test]
fn nothing_to_update_says_so_with_the_time_of_the_check() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(Found { at: AT, repo: Ok(Vec::new()), aur: None }));
    let screen = h.screen();
    for text in ["Everything is up to date", "Last checked 14:02.", "Check now"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.set_locale("tr");
    let screen = h.screen();
    for text in ["Her şey güncel", "Son kontrol 14:02.", "Şimdi kontrol et"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn the_list_stays_while_a_check_runs_and_a_failed_check_keeps_it() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(found()));
    h.send(Msg::CheckNow);
    let screen = h.screen();
    assert!(screen.contains("linux") && screen.contains("checking"), "{screen}");
    let failure = Failure::Said("error: failed retrieving file 'core.db'".to_owned());
    h.send(Msg::Checked(Found { at: AT + 600, repo: Err(failure), aur: None }));
    let screen = h.screen();
    assert!(screen.contains("linux"), "the list stays:\n{screen}");
    assert!(screen.contains("The last check did not finish: error: failed retrieving file"), "{screen}");
    assert!(screen.contains("last checked 14:02"), "the time is the last check that got through:\n{screen}");
    assert!(!screen.contains("visual-studio-code-bin"), "the AUR was not asked this time:\n{screen}");
}

#[test]
fn a_second_check_does_not_start_while_one_runs() {
    let mut updates = Updates::default();
    assert_eq!(updates.update(Msg::CheckNow), Some(Request::Check));
    assert_eq!(updates.update(Msg::CheckNow), None);
    let _ = updates.update(Msg::Checked(found()));
    assert_eq!(updates.update(Msg::CheckNow), Some(Request::Check), "once it ended, another may start");
}

#[test]
fn a_first_check_that_fails_says_why_and_offers_another() {
    let mut h = tab(100, 20);
    let screen = h.screen();
    assert!(screen.contains("Looking for updates"), "before any answer:\n{screen}");
    h.send(Msg::Checked(Found { at: AT, repo: Err(Failure::NoFakeroot), aur: None }));
    let screen = h.screen();
    for text in ["Could not check for updates", "fakeroot", "base-devel", "Check now"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.click_text("Check now");
    assert!(h.screen().contains("Looking for updates"), "the check runs again:\n{}", h.screen());
}

#[test]
fn a_turned_off_aur_is_neither_shown_nor_counted() {
    let cx = Cx { aur: false, utc_offset: 0, can_check: true };
    let mut h = Harness::with_env(Tab(Updates::default(), cx), crate::test_env(), 100, 20);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    h.send(Msg::Checked(found()));
    let screen = h.screen();
    assert!(screen.contains("2 updates") && !screen.contains("visual-studio"), "{screen}");
    assert_eq!(h.app().0.pending(false), 2);
    assert_eq!(h.app().0.pending(true), 3);
}

#[test]
fn the_arrow_follows_the_glyph_mode_and_ascii_keeps_the_rules() {
    for (width, height) in [(36, 14), (60, 20), (100, 24)] {
        let mut h = tab(width, height);
        h.send(Msg::Checked(found()));
        h.set_glyph_mode(GlyphMode::Nerd);
        assert!(h.screen().contains('\u{f061}'), "{}", h.screen());
        h.set_glyph_mode(GlyphMode::Ascii);
        let screen = h.screen();
        let kernel = screen.lines().find(|line| line.contains("linux")).expect("the kernel's row");
        assert!(
            kernel.contains("6.18.1-1") && kernel.contains(">") && kernel.contains("6.18.2-1"),
            "never cut:\n{screen}"
        );
        assert_eq!(kernel.contains("restart needed"), width >= 100, "the note only where it fits:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|', '→'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
        }
        assert!(!screen.contains("-->"), "{screen}");
    }
}

#[test]
fn times_are_written_where_the_machine_stands() {
    assert_eq!(clock(AT, 0), "14:02");
    assert_eq!(clock(AT, 180), "17:02", "Istanbul");
    assert_eq!(clock(AT, -900), "23:02", "an offset can cross midnight");
}

/// The application on a pretend machine whose repositories have the kernel and mesa waiting,
/// and the AUR a code editor, as pacman and paru would print them.
fn machine(settings: &str) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("updates", &[Sample::new("linux", "6.18.1-1", "The Linux kernel")]);
    scratch.repository("core");
    let work = scratch.check();
    let recorded = Arc::new(Recorded::default());
    recorded.answer(FAKEROOT, &command::refresh(&work), "", 0);
    recorded.answer(
        PACMAN,
        &command::update_check(&work),
        "linux 6.18.1-1 -> 6.18.2-1\nmesa 25.2.3-1 -> 25.2.4-1\n",
        0,
    );
    recorded.answer("paru", &command::aur_update_check(), "visual-studio-code-bin 1.104.0-1 -> 1.105.0-1\n", 0);
    let settings = Settings::parse_str("packages.conf", settings);
    let mut h = Harness::with_env(app_in(&scratch, settings, &recorded), crate::test_env(), 120, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.advance(Duration::from_millis(20));
    (h, scratch, recorded)
}

#[test]
fn the_application_checks_once_it_knows_its_sources_and_counts_on_the_tab() {
    let (mut h, scratch, recorded) = machine("");
    let header = h.screen().lines().next().unwrap_or_default().to_owned();
    assert!(header.contains('●') && header.contains(" 3 "), "{header}");
    h.send(AppMsg::Tab(crate::app::Tab::Updates.index()));
    let screen = h.screen();
    for text in ["3 updates · last checked", "Repositories", "linux", "mesa", "AUR", "visual-studio-code-bin"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let lines = recorded.command_lines();
    assert!(!lines.iter().any(|line| line.contains("-Syu") || line.starts_with("pacman -Sy")), "{lines:?}");
    assert!(
        lines.iter().any(|line| line.starts_with("fakeroot -- pacman -Sy --dbpath") && line.contains("check")),
        "only the private copy is refreshed: {lines:?}"
    );
    assert!(scratch.check().join("sync/core.db").exists(), "the copy is where qpac keeps it");
    h.click_text("Check now");
    h.advance(Duration::from_millis(20));
    let refreshes = recorded.command_lines().iter().filter(|line| line.starts_with("fakeroot")).count();
    assert_eq!(refreshes, 2, "the button checks again");
}

#[test]
fn a_turned_off_aur_is_not_asked() {
    let (mut h, _scratch, recorded) = machine("[sources]\naur = false\n");
    h.send(AppMsg::Tab(crate::app::Tab::Updates.index()));
    let screen = h.screen();
    assert!(screen.contains("2 updates") && !screen.contains("visual-studio"), "{screen}");
    assert!(!recorded.command_lines().iter().any(|line| line.starts_with("paru")), "{:?}", recorded.command_lines());
}

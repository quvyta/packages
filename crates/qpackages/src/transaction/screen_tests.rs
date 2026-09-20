//! The running state on screen. A recorded stream plays to its end inside one harness step, so
//! the moments while pacman runs are shown through a small application around the flow alone,
//! fed the same messages the task would send.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use qframe::event::{KeyEvent, KeyKind};
use qframe::icons::{GlyphMode, PILLAR};
use qframe::runtime::{Task, TaskEvent, TaskOutcome};

use super::*;
use crate::helper::session::InProcess;
use crate::runner::Recorded;

/// A read of nowhere, for a flow that never gets to reread.
fn no_reload() -> Reload {
    Reload::new(
        Path::new("/nowhere/local"),
        Path::new("/nowhere/applications"),
        qpackages_core::sources::AurPreference::default(),
        Arc::new(|_| None),
        Arc::new(Recorded::default()),
    )
}

/// The flow's own screen: its output pane and its dialogs, nothing else.
struct Pane(Flow);

impl App for Pane {
    type Msg = AppMsg;

    fn update(&mut self, msg: AppMsg) -> Command<AppMsg> {
        match msg {
            AppMsg::Transaction(msg) => self.0.update(msg, (80, 24)),
            _ => Command::none(),
        }
    }

    fn view(&self, ui: &mut View<'_, AppMsg>) {
        view::output(&self.0, ui);
        view::modal(&self.0, &[], ui);
    }
}

/// An idle flow whose helpers would run as root and play nothing.
fn idle() -> Flow {
    let recorded = Arc::new(Recorded::default());
    let session = Session::new(InProcess::new(&recorded, 0).start_fn());
    Flow::new(recorded, session, Tool::Sudo, no_reload(), Path::new("/nowhere"))
}

/// A flow in the middle of installing paru, as `execute` leaves it, without a process behind it.
fn running() -> (Flow, TaskId) {
    let mut flow = idle();
    let task: Task<AppMsg> = Task::new("never started", |_| Ok(wrap(Msg::Cancel)));
    let id = task.id();
    flow.state =
        State::Running { job: Job::new(Action::Install(vec!["paru".to_owned()]), backup::Plan::Off), task: id };
    (flow, id)
}

/// The environment, loaded once per test thread and handed out as a copy. Loading it parses
/// every language file, the key bindings and the icon set, which the checks that walk nine
/// languages at three widths would otherwise pay for hundreds of times.
fn shared_env() -> qframe::env::Env {
    thread_local! {
        static LOADED: qframe::env::Env = crate::locales::env();
    }
    LOADED.with(Clone::clone)
}

fn harness(flow: Flow, width: u16, height: u16) -> Harness<Pane> {
    let mut h = Harness::with_env(Pane(flow), shared_env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h
}

fn line(text: &str) -> AppMsg {
    wrap(Msg::Line(text.to_owned()))
}

#[test]
fn lines_stream_into_the_pane_and_the_step_counter_moves_the_bar() {
    let (flow, _) = running();
    let mut h = harness(flow, 80, 12);
    let screen = h.screen();
    for text in ["Installing paru", "Hold to stop", "Waiting for pacman"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.send(line("\u{1b}[?25l:: Retrieving packages..."));
    h.send(line(" paru-2.1.0-1-x86_64   1.2 MiB  2.4 MiB/s 00:01"));
    let screen = h.screen();
    assert!(screen.contains(":: Retrieving packages..."), "{screen}");
    assert!(!screen.contains("Waiting for pacman"), "{screen}");
    assert!(!screen.contains('\u{1b}'), "{screen}");
    assert!(!screen.contains('%'), "the bar is indeterminate before a step counter:\n{screen}");
    h.send(line("(1/4) checking keys in keyring"));
    assert!(h.screen().contains("25%"), "{}", h.screen());
    h.send(line("(3/4) loading package files"));
    assert!(h.screen().contains("75%"), "{}", h.screen());
    match h.app().0.state() {
        State::Running { job, .. } => {
            assert_eq!(job.progress, Some(0.75));
            assert_eq!(job.output.len(), 4, "every line with words is kept");
        }
        other => panic!("still running: {other:?}"),
    }
}

#[test]
fn holding_stop_cancels_the_task_and_the_stopped_task_leaves_the_pane_open() {
    let (flow, id) = running();
    let mut h = harness(flow, 80, 12);
    h.send(line("(1/4) checking keys in keyring"));
    h.click_text("Hold to stop");
    assert!(matches!(h.app().0.state(), State::Running { .. }), "a click is not a hold");
    h.key(KeyEvent::press("enter"));
    for _ in 0..45 {
        h.advance(Duration::from_millis(30));
        h.key(KeyEvent { kind: KeyKind::Repeat, ..KeyEvent::press("enter") });
    }
    // The runtime answers the cancellation with the task's outcome; until then pacman may
    // still be printing, and the pane keeps saying so.
    assert!(matches!(h.app().0.state(), State::Running { .. }), "the flow waits for the outcome");
    h.send(wrap(Msg::Event(TaskEvent::Finished { id, outcome: TaskOutcome::Cancelled })));
    h.advance(Duration::from_millis(50));
    let screen = h.screen();
    for text in
        ["Stopped", "may run to its end", "Installing paru did not go through", "checking keys in keyring", "Close"]
    {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Hold to stop"), "{screen}");
    h.click_text("Close");
    assert!(matches!(h.app().0.state(), State::Idle));
    assert!(!h.screen().contains("checking keys"), "{}", h.screen());
}

#[test]
fn a_task_that_could_not_start_pacman_says_why() {
    let (flow, id) = running();
    let mut h = harness(flow, 80, 12);
    let outcome = TaskOutcome::Failed("No such file or directory (os error 2)".to_owned());
    h.send(wrap(Msg::Event(TaskEvent::Finished { id, outcome })));
    h.advance(Duration::from_millis(50));
    let screen = h.screen();
    assert!(screen.contains("pacman could not be started"), "{screen}");
    assert!(screen.contains("os error 2"), "{screen}");
    assert!(matches!(h.app().0.state(), State::Finished { .. }));
}

/// A flow in the middle of removing a Flatpak for the user, as `run_flatpak` leaves it.
fn removing_flatpak() -> (Flow, TaskId) {
    let mut flow = idle();
    let task: Task<AppMsg> = Task::new("never started", |_| Ok(wrap(Msg::Cancel)));
    let id = task.id();
    let job = Job::new(Action::FlatpakRemove(vec!["org.gimp.GIMP".to_owned()]), backup::Plan::Off);
    flow.state = State::Running { job, task: id };
    (flow, id)
}

#[test]
fn a_flatpak_run_names_flatpak_its_steps_and_its_stop_in_both_languages() {
    let (flow, id) = removing_flatpak();
    // Wide enough for the Turkish heading beside the stop control, which is the wider of the two
    // languages; what a pane too narrow for both does is held by the checks further down.
    let mut h = harness(flow, 120, 12);
    let screen = h.screen();
    for text in ["Removing org.gimp.GIMP · 1/2 · removing the apps", "Waiting for Flatpak"] {
        assert!(screen.contains(text), "`{text}`:\n{screen}");
    }
    h.set_locale("tr");
    let screen = h.screen();
    for text in ["org.gimp.GIMP kaldırılıyor · 1/2 · uygulamalar kaldırılıyor", "Flatpak bekleniyor"] {
        assert!(screen.contains(text), "`{text}`:\n{screen}");
    }
    h.set_locale("en");
    h.send(line("Uninstalling app/org.gimp.GIMP/x86_64/stable"));
    h.send(wrap(Msg::Event(TaskEvent::Finished { id, outcome: TaskOutcome::Cancelled })));
    h.advance(Duration::from_millis(50));
    let screen = h.screen();
    for text in ["Stopped", "Flatpak was stopped", "Uninstalling app/org.gimp.GIMP"] {
        assert!(screen.contains(text), "`{text}`:\n{screen}");
    }
    assert!(!screen.contains("pacman"), "{screen}");
}

#[test]
fn a_flatpak_that_could_not_start_says_so() {
    let (flow, id) = removing_flatpak();
    let mut h = harness(flow, 80, 12);
    let outcome = TaskOutcome::Failed("No such file or directory (os error 2)".to_owned());
    h.send(wrap(Msg::Event(TaskEvent::Finished { id, outcome })));
    h.advance(Duration::from_millis(50));
    let screen = h.screen();
    assert!(screen.contains("Flatpak could not be started"), "{screen}");
}

#[test]
fn events_of_another_task_and_lines_after_the_end_are_ignored() {
    let (flow, _) = running();
    let other: Task<AppMsg> = Task::new("another", |_| Ok(wrap(Msg::Cancel)));
    let mut h = harness(flow, 80, 12);
    h.send(wrap(Msg::Event(TaskEvent::Finished { id: other.id(), outcome: TaskOutcome::Cancelled })));
    assert!(matches!(h.app().0.state(), State::Running { .. }), "another task's end changes nothing");
    h.send(wrap(Msg::Ended(ProcessOutcome::Finished { code: Some(2) })));
    h.send(line("late"));
    match h.app().0.state() {
        State::Finished(job) => assert_eq!(job.output.len(), 0, "a line after the end is dropped"),
        other => panic!("finished: {other:?}"),
    }
}

#[test]
fn the_pane_is_drawn_on_a_narrow_screen_in_ascii_without_decoration() {
    let (flow, _) = running();
    let mut h = harness(flow, 60, 8);
    h.set_glyph_mode(GlyphMode::Ascii);
    h.send(line("(2/4) checking package integrity"));
    let screen = h.screen();
    assert!(screen.contains("50%"), "{screen}");
    for forbidden in ['[', ']', '{', '}', '|'] {
        assert!(!screen.contains(forbidden), "`{forbidden}`:\n{screen}");
    }
}

/// A flow showing the confirmation of `action` with a one-package plan, the helper up or not.
fn confirming(action: Action, granted: bool) -> Flow {
    let mut flow = idle();
    let plan = if action.is_removal() {
        qpackages_core::pacman::parse_remove_plan("yay|12.5.0-1\n")
    } else {
        qpackages_core::pacman::parse_install_plan("extra|paru|2.1.0-1|1258291\n")
    };
    flow.state = State::Confirming {
        action,
        plan,
        granted,
        backup: backup::Plan::Off,
        pending: 0,
        then: Vec::new(),
        build: None,
    };
    flow
}

#[test]
fn the_confirmation_says_whether_permission_will_be_asked_in_both_languages() {
    let install = || Action::Install(vec!["paru".to_owned()]);
    for (granted, en, tr) in [
        (
            false,
            ["Administrator permission will be asked;", "qpac closes."],
            ["Yönetici izni istenecek;", "kadar geçerli."],
        ),
        (true, ["Administrator permission was given", "session."], ["Yönetici izni bu oturumda verildi.", ""]),
    ] {
        // The dialog is narrow and the sentence wraps; each part lands on a line of its own.
        let mut h = harness(confirming(install(), granted), 100, 16);
        let screen = h.screen();
        assert!(en.iter().all(|part| screen.contains(part)), "{screen}");
        h.set_locale("tr");
        let screen = h.screen();
        assert!(tr.iter().all(|part| screen.contains(part)), "{screen}");
        assert!(!h.screen().contains("Ayar dosyaları"), "an installation deletes nothing:\n{}", h.screen());
    }
}

#[test]
fn a_removal_says_the_settings_files_go_too() {
    let mut h = harness(confirming(Action::Remove(vec!["yay".to_owned()]), false), 100, 16);
    assert!(h.screen().contains("Their settings files are deleted too."), "{}", h.screen());
    h.set_locale("tr");
    assert!(h.screen().contains("Ayar dosyaları da silinir."), "{}", h.screen());
}

/// A flow in the middle of a system update wrapped in snapper snapshots, past the first one.
fn updating() -> Flow {
    let mut flow = idle();
    let task: Task<AppMsg> = Task::new("never started", |_| Ok(wrap(Msg::Cancel)));
    let update = qpackages_core::pacman::Update {
        name: "linux".to_owned(),
        from: "6.18.1-1".to_owned(),
        to: "6.18.2-1".to_owned(),
        ignored: false,
    };
    let mut job = Job::new(Action::Upgrade(vec![update]), backup::Plan::Take(backup::Tool::Snapper));
    job.line("42");
    assert!(job.took_before());
    job.advance();
    flow.state = State::Running { job, task: task.id() };
    flow
}

#[test]
fn the_heading_steps_through_an_update_in_every_glyph_mode_and_both_languages() {
    let mut h = harness(updating(), 100, 10);
    for mode in [GlyphMode::Unicode, GlyphMode::Ascii, GlyphMode::Nerd] {
        h.set_glyph_mode(mode);
        h.set_locale("en");
        let screen = h.screen();
        assert!(screen.contains("Updating the system · 2/3 · installing packages"), "{mode:?}:\n{screen}");
        assert!(screen.contains("42"), "the first snapshot's output stays:\n{screen}");
        h.set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Sistem güncelleniyor · 2/3 · paketler kuruluyor"), "{mode:?}:\n{screen}");
    }
    h.set_locale("en");
    h.send(line("(1/2) upgrading linux"));
    assert!(h.screen().contains("50%"), "{}", h.screen());
    h.resize(50, 8);
    let screen = h.screen();
    // Too narrow for both: the control that stops the run keeps its whole width, since a stop
    // half drawn cannot be pressed with confidence, and the heading is the one that shortens.
    assert!(screen.contains("Hold to stop"), "the stop stays whole on a narrow pane:\n{screen}");
    assert!(screen.contains("Updating the"), "the heading keeps its start:\n{screen}");
}

#[test]
fn the_pacnew_list_waits_for_the_pane_and_closes_on_its_own() {
    let mut flow = updating();
    flow.state = State::Idle;
    flow.pacnew = Some(vec!["/etc/pacman.conf.pacnew".to_owned(), "/etc/locale.gen.pacnew".to_owned()]);
    let mut h = harness(flow, 80, 16);
    for mode in [GlyphMode::Unicode, GlyphMode::Ascii, GlyphMode::Nerd] {
        h.set_glyph_mode(mode);
        let screen = h.screen();
        for text in ["2 new configuration files", "/etc/pacman.conf.pacnew", "/etc/locale.gen.pacnew"] {
            assert!(screen.contains(text), "`{text}` in {mode:?}:\n{screen}");
        }
    }
    h.set_locale("tr");
    assert!(h.screen().contains("2 yeni yapılandırma dosyası"), "{}", h.screen());
    h.press("esc");
    assert!(h.app().0.pacnew().is_none());
}

/// The widths the dialogs and the output pane are held to. A dialog is 56 cells wide and keeps a
/// column of dimmed screen either side, so 60 is the narrowest screen it stands on whole.
const NARROW: [u16; 3] = [60, 80, 120];

/// A screen tall enough that nothing is left out for want of room below, so width alone is what
/// the language checks put under test.
const TALL: u16 = 40;

/// The text `key` holds in the language `code`, read from the language file itself and kept.
///
/// Every lookup parses all nine files afresh, and the checks below ask for a few hundred of
/// them, so each answer is kept for the ones that follow.
fn label(code: &str, key: &str) -> String {
    static KNOWN: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Mutex::default);
    let asked = (code.to_owned(), key.to_owned());
    if let Some(text) = known.lock().unwrap_or_else(PoisonError::into_inner).get(&asked) {
        return text.clone();
    }
    let text = crate::locales::tests::label(code, key);
    known.lock().unwrap_or_else(PoisonError::into_inner).insert(asked, text.clone());
    text
}

/// A flow that found the database locked and cannot say who holds it, which is the notice with
/// the most words in it.
fn locked_flow() -> Flow {
    let mut flow = idle();
    flow.state = State::Locked { since: None, owner: None };
    flow
}

/// A confirmation of `action` with `pending` updates waiting behind it.
fn confirming_with(action: Action, granted: bool, pending: usize) -> Flow {
    let mut flow = confirming(action, granted);
    if let State::Confirming { pending: waiting, .. } = &mut flow.state {
        *waiting = pending;
    }
    flow
}

/// The `.pacnew` files a transaction left, which the flow shows until it is closed.
fn pacnew_flow() -> Flow {
    let mut flow = idle();
    flow.pacnew = Some(vec!["/etc/pacman.conf.pacnew".to_owned()]);
    flow
}

/// Every dialog the flow puts on screen, with the keys of the buttons at its bottom. Buttons are
/// what a long translation breaks first: they stand side by side on one row at the bottom right,
/// and each must be read whole to be pressed with confidence.
fn dialogs() -> Vec<(Flow, &'static [&'static str])> {
    let update =
        Update { name: "linux".to_owned(), from: "6.18.1-1".to_owned(), to: "6.18.2-1".to_owned(), ignored: false };
    let confirmations: Vec<(Action, usize, &'static [&'static str])> = vec![
        (Action::Install(vec!["paru".to_owned()]), 0, &["transaction.cancel", "transaction.install"]),
        (
            Action::Install(vec!["paru".to_owned()]),
            2,
            &["transaction.cancel", "transaction.install-only", "transaction.upgrade-and-install"],
        ),
        (Action::Remove(vec!["yay".to_owned()]), 0, &["transaction.cancel", "transaction.remove"]),
        (Action::Upgrade(vec![update]), 0, &["transaction.cancel", "transaction.upgrade"]),
        (Action::AddFlathub, 0, &["transaction.cancel", "flatpak.add"]),
        (Action::AurInstall(vec!["paru".to_owned()]), 0, &["transaction.cancel", "aur.build"]),
        (Action::SnapRefresh(vec!["vlc".to_owned()]), 0, &["transaction.cancel", "snap.refresh"]),
        (Action::SnapLink, 0, &["transaction.cancel", "snap.link"]),
        (Action::SnapdSocket(true), 0, &["transaction.cancel", "snap.socket-on"]),
        (Action::SnapdSocket(false), 0, &["transaction.cancel", "snap.socket-off"]),
        (Action::Timer(true), 0, &["transaction.cancel", "mirrors.timer-on"]),
        (Action::Timer(false), 0, &["transaction.cancel", "mirrors.timer-off"]),
    ];
    let mut all: Vec<(Flow, &'static [&'static str])> = confirmations
        .into_iter()
        .map(|(action, pending, keys)| (confirming_with(action, false, pending), keys))
        .collect();
    all.push((locked_flow(), &["lock.close"]));
    all.push((pacnew_flow(), &["transaction.close"]));
    all
}

#[test]
fn the_lock_notice_shows_its_fixed_labels_whole_in_every_language() {
    for code in crate::locales::tests::codes() {
        for width in NARROW {
            let mut h = harness(locked_flow(), width, TALL);
            h.set_locale(&code);
            let screen = h.screen();
            for key in ["lock.held", "lock.close"] {
                let text = label(&code, key);
                assert!(
                    screen.contains(&text),
                    "`{key}` is cut off in `{code}` at {width} columns; it reads `{text}`:\n{screen}"
                );
            }
        }
    }
}

#[test]
fn the_pane_of_a_run_and_of_a_failed_run_shows_its_fixed_labels_whole_in_every_language() {
    for code in crate::locales::tests::codes() {
        for width in NARROW {
            let (flow, id) = running();
            let mut h = harness(flow, width, 12);
            h.set_locale(&code);
            let screen = h.screen();
            // While it runs: the control that stops it, and what the empty output pane says.
            for key in ["transaction.stop", "transaction.waiting"] {
                let text = label(&code, key);
                assert!(
                    screen.contains(&text),
                    "`{key}` is cut off in `{code}` at {width} columns while a run is on:\n{screen}"
                );
            }
            h.send(line("(1/4) checking keys in keyring"));
            h.send(wrap(Msg::Event(TaskEvent::Finished { id, outcome: TaskOutcome::Cancelled })));
            h.advance(Duration::from_millis(50));
            let screen = h.screen();
            let text = label(&code, "transaction.close");
            assert!(
                screen.contains(&text),
                "`transaction.close` is cut off in `{code}` at {width} columns after a failed run:\n{screen}"
            );
        }
    }
}

/// The dialog's frame on the drawn screen: its first and last column, its first and last row.
///
/// A dialog has no border characters; its edge is where its own surface colour ends. The pillar
/// runs down the whole left edge of that surface, which names the first column and every row the
/// surface covers, and the run of surface colour along the bottom row names the last column.
fn frame(h: &Harness<Pane>) -> (u16, u16, u16, u16) {
    let pillar = h.env().icons().glyph(PILLAR).into_owned();
    let area = h.buffer().area;
    let mut cells = Vec::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if h.buffer()[(x, y)].symbol() == pillar {
                cells.push((x, y));
            }
        }
    }
    assert!(!cells.is_empty(), "a dialog draws a pillar down its left edge:\n{}", h.screen());
    let (left, top) = cells[0];
    let (last, bottom) = cells[cells.len() - 1];
    assert_eq!(left, last, "the pillar stands in one column:\n{}", h.screen());
    let surface = h.bg(left, bottom);
    let mut right = left;
    while right + 1 < area.width && h.bg(right + 1, bottom) == surface {
        right += 1;
    }
    assert!(right > left + 2, "the dialog is wider than its padding:\n{}", h.screen());
    (left, top, right, bottom)
}

/// Every place `text` is drawn on screen: its row, and the first and last column it covers.
fn places(h: &Harness<Pane>, text: &str) -> Vec<(u16, u16, u16)> {
    let area = h.buffer().area;
    let cells = qframe::text::width(text).max(1) - 1;
    let mut found = Vec::new();
    for y in 0..area.height {
        let mut line = String::new();
        let mut columns = Vec::new();
        for x in 0..area.width {
            let symbol = h.buffer()[(x, y)].symbol();
            columns.extend(std::iter::repeat_n(x, symbol.len()));
            line.push_str(symbol);
        }
        let mut from = 0;
        while let Some(at) = line[from..].find(text) {
            let start = from + at;
            found.push((y, columns[start], columns[start] + cells));
            from = start + text.len();
        }
    }
    found
}

/// Nothing but the pillar and the close mark stands on the frame's own edge. A word that reaches
/// it has left the panel, and the next cell out is the dimmed screen.
fn nothing_stands_on_the_border(h: &Harness<Pane>, (left, top, right, bottom): (u16, u16, u16, u16)) {
    let pillar = h.env().icons().glyph(PILLAR).into_owned();
    let close = h.env().icons().glyph("close").into_owned();
    let mut edge: Vec<(u16, u16)> = (top..=bottom).flat_map(|y| [(left, y), (right, y)]).collect();
    edge.extend((left..=right).flat_map(|x| [(x, top), (x, bottom)]));
    for (x, y) in edge {
        let symbol = h.buffer()[(x, y)].symbol();
        // The close mark takes the three cells at the right end of the first row.
        let marked = y == top && x + 3 > right;
        let allowed =
            symbol.trim().is_empty() || (x == left && symbol == pillar) || (marked && (symbol == close || x != right));
        assert!(allowed, "`{symbol}` stands on the dialog's edge at column {x}, row {y}:\n{}", h.screen());
    }
}

#[test]
fn every_button_of_a_dialog_stands_whole_inside_its_frame_in_every_language() {
    // Two things are asked of every button, in every language the application carries.
    //
    // It must be there whole: a label the dialog cannot fit is shortened with an ellipsis rather
    // than left out, so a screen that merely draws something is no proof. Each label is read out
    // of the language file and looked for as it is written, and a shortened "Installieren" is no
    // longer that string.
    //
    // And it must sit inside the panel it belongs to. A label that reaches the frame is drawn
    // over the dialog's own edge, and one that runs past it lands on the dimmed screen, where it
    // reads as part of the page behind.
    for code in crate::locales::tests::codes() {
        for width in NARROW {
            for (flow, keys) in dialogs() {
                let mut h = harness(flow, width, TALL);
                h.set_locale(&code);
                let bounds = frame(&h);
                let (left, top, right, bottom) = bounds;
                for key in keys {
                    let text = label(&code, key);
                    let drawn = places(&h, &text);
                    assert!(
                        !drawn.is_empty(),
                        "`{key}` is not on screen in `{code}` at {width} columns; it reads `{text}`:\n{}",
                        h.screen()
                    );
                    for (y, first, last) in drawn {
                        assert!(
                            y > top && y < bottom && first > left && last < right,
                            "`{key}` in `{code}` at {width} columns lies at columns {first}–{last} of row {y}, \
                             outside the dialog's frame {left}–{right} by {top}–{bottom}:\n{}",
                            h.screen()
                        );
                    }
                }
                nothing_stands_on_the_border(&h, bounds);
            }
        }
    }
}

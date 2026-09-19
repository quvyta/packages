//! The running state on screen. A recorded stream plays to its end inside one harness step, so
//! the moments while pacman runs are shown through a small application around the flow alone,
//! fed the same messages the task would send.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use qframe::event::{KeyEvent, KeyKind};
use qframe::icons::GlyphMode;
use qframe::runtime::{Task, TaskEvent, TaskOutcome};
use qframe::widgets::LogBuffer;

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
        view::modal(&self.0, ui);
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
    flow.state = State::Running {
        action: Action::Install(vec!["paru".to_owned()]),
        task: id,
        output: LogBuffer::new(OUTPUT_LINES),
        progress: None,
    };
    (flow, id)
}

fn harness(flow: Flow, width: u16, height: u16) -> Harness<Pane> {
    let mut h = Harness::with_env(Pane(flow), crate::test_env(), width, height);
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
        State::Running { progress, output, .. } => {
            assert_eq!(*progress, Some(0.75));
            assert_eq!(output.len(), 4, "every line with words is kept");
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
        State::Finished { output, .. } => assert_eq!(output.len(), 0, "a line after the end is dropped"),
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
    flow.state = State::Confirming { action, plan, granted };
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

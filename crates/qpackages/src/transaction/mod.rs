//! A pacman transaction from the user's decision to its result: plan without privileges, confirm
//! on our screen, have the root helper run pacman on a pseudo-terminal while the list stays where
//! it is, then say how it went.
//!
//! The first transaction of a run starts the helper, which stays up until qpac quits. Where polkit
//! is installed, pkexec asks for the password while the terminal is handed over and starts the
//! helper itself; elsewhere sudo asks on the real terminal, then `sudo -n` starts it. Later
//! transactions go straight to it. No password ever passes through this code.

pub mod filter;
#[cfg(test)]
mod screen_tests;
pub mod view;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

use qframe::prelude::*;
use qframe::runtime::{
    ChildLine, DetachedHandoff, DetachedOutcome, Handoff, HandoffOutcome, ProcessOutcome, Task, TaskCx, TaskEvent,
    TaskId, TaskOutcome,
};
use qframe::widgets::{LogBuffer, LogLevel, LogLine, Toast};
use qpackages_core::helper::{Refusal, Request};
use qpackages_core::lock::{LockStatus, Owner, lock_status};
use qpackages_core::pacman::command::{self, PACMAN, SUDO};
use qpackages_core::pacman::{Plan, parse_install_plan, parse_remove_plan};

use crate::app::Msg as AppMsg;
use crate::helper::pkexec::{self, Tool};
use crate::helper::session::{self, Outcome, Session, StartFailure, Wait};
use crate::reload::Reload;
use crate::runner::Runner;

/// Lines of output kept; the oldest fall out once pacman has said more than this.
const OUTPUT_LINES: usize = 50_000;

/// The key every toast of the flow shares, so a later one replaces the earlier in place.
const TOAST_KEY: &str = "transaction";

/// What the user asked pacman to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Install these packages from the repositories.
    Install(Vec<String>),
    /// Remove these packages with the dependencies only they needed and their saved settings.
    Remove(Vec<String>),
}

impl Action {
    /// The packages named.
    #[must_use]
    pub fn names(&self) -> &[String] {
        match self {
            Self::Install(names) | Self::Remove(names) => names,
        }
    }

    /// Whether the action takes packages away.
    #[must_use]
    pub fn is_removal(&self) -> bool {
        matches!(self, Self::Remove(_))
    }

    /// The names as one line, for toasts and headings.
    #[must_use]
    pub fn names_text(&self) -> String {
        self.names().join(", ")
    }

    /// The pacman arguments that only print the plan.
    fn print_args(&self) -> Vec<String> {
        match self {
            Self::Install(names) => command::print_install(names),
            Self::Remove(names) => command::print_remove(names),
        }
    }

    /// The request that has the helper carry the action out.
    fn request(&self) -> Request {
        match self {
            Self::Install(names) => Request::Install(names.clone()),
            Self::Remove(names) => Request::Remove(names.clone()),
        }
    }

    /// Reads the plan pacman printed for this action.
    fn parse_plan(&self, text: &str) -> Plan {
        match self {
            Self::Install(_) => parse_install_plan(text),
            Self::Remove(_) => parse_remove_plan(text),
        }
    }
}

/// Everything the planning step found out, delivered in one message so the screen decides once.
#[derive(Debug, Clone)]
pub struct Planned {
    action: Action,
    /// The plan, or what pacman said when it could not make one.
    plan: Result<Plan, String>,
    /// Whether the helper is up, so no password will be asked.
    granted: bool,
    lock: LockStatus,
}

/// Everything that can happen in the flow.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The user asked for an action; planning starts.
    Begin(Action),
    /// Planning ended.
    Planned(Planned),
    /// The user closed the confirmation or the lock notice.
    Cancel,
    /// The user confirmed the plan.
    Apply,
    /// sudo's ticket was warmed on the real terminal, or not.
    Warmed(HandoffOutcome),
    /// pkexec gave the terminal back: the helper said its first line, or pkexec ended without
    /// starting it, or could not run.
    HelperStarted(DetachedOutcome),
    /// A helper pkexec started said something after its first line; the number tells which
    /// start it came from, so a helper let go earlier cannot speak for the current one.
    HelperLine(u64, ChildLine),
    /// The helper is up, or why it is not.
    Started(Result<(), StartFailure>),
    /// pacman printed a line.
    Line(String),
    /// pacman ended.
    Ended(ProcessOutcome),
    /// The helper refused the request; nothing ran.
    Refused(Refusal),
    /// The helper went away before pacman's end was known; what was known about it.
    Lost(String),
    /// The user let the helper go.
    EndHelper,
    /// The task running pacman started, progressed or ended.
    Event(TaskEvent),
    /// The user held the stop control.
    Stop,
    /// The user closed the output pane.
    Close,
    /// The user dragged the output pane to this many rows.
    OutputHeight(u16),
}

/// Where the flow is.
#[derive(Debug)]
pub enum State {
    /// Nothing is going on.
    Idle,
    /// pacman is being asked what the action would do.
    Planning(Action),
    /// The plan is on screen, waiting for the user.
    Confirming {
        /// The action planned.
        action: Action,
        /// What pacman would do.
        plan: Plan,
        /// Whether the helper is up, so no password will be asked.
        granted: bool,
    },
    /// Another transaction holds pacman's database; nothing can start.
    Locked {
        /// When the lock was taken, when the file said.
        since: Option<SystemTime>,
        /// The process holding it, when it could be seen.
        owner: Option<Owner>,
    },
    /// sudo or pkexec has the terminal and is asking for the password.
    Authorizing(Action),
    /// The helper is starting.
    Starting(Action),
    /// pacman is running.
    Running {
        /// The action being carried out.
        action: Action,
        /// The task streaming pacman's output.
        task: TaskId,
        /// What pacman said so far.
        output: LogBuffer,
        /// The step counter's share, once pacman printed one.
        progress: Option<f32>,
    },
    /// pacman ended without success; its output stays on screen until closed.
    Finished {
        /// The action that was attempted.
        action: Action,
        /// What pacman said.
        output: LogBuffer,
    },
}

/// The flow's state and what it needs to run programs.
pub struct Flow {
    runner: Arc<dyn Runner>,
    /// The root helper of this run.
    session: Session,
    /// The program that asks for permission before the helper starts.
    tool: Tool,
    /// How many times pkexec was asked to start a helper; each start's lines carry its number.
    starts: u64,
    /// Where the lines of the helper pkexec started go, with its start's number, until it ends.
    helper_lines: Option<(u64, Sender<io::Result<String>>)>,
    /// Whether the helper is up, as last seen; the header shows it without waiting on the helper.
    granted: bool,
    /// The read of the packages and the sources, repeated after a successful transaction.
    reload: Reload,
    /// The directory pacman's lock file lives in.
    lock_dir: PathBuf,
    state: State,
    /// Rows the output pane takes, as the user dragged it.
    output_height: u16,
}

impl fmt::Debug for Flow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flow")
            .field("reload", &self.reload)
            .field("lock_dir", &self.lock_dir)
            .field("state", &self.state)
            .field("output_height", &self.output_height)
            .finish_non_exhaustive()
    }
}

/// Wraps a flow message for the application.
fn wrap(msg: Msg) -> AppMsg {
    AppMsg::Transaction(msg)
}

impl Flow {
    /// An idle flow that plans with `runner`, carries out through `session`, whose helper `tool`
    /// asks permission for, checks the lock under `lock_dir` and repeats `reload` after a change.
    #[must_use]
    pub fn new(runner: Arc<dyn Runner>, session: Session, tool: Tool, reload: Reload, lock_dir: &Path) -> Self {
        Self {
            runner,
            session,
            tool,
            starts: 0,
            helper_lines: None,
            granted: false,
            reload,
            lock_dir: lock_dir.to_path_buf(),
            state: State::Idle,
            output_height: 10,
        }
    }

    /// The program that asks for permission before the helper starts.
    #[must_use]
    pub fn tool(&self) -> &Tool {
        &self.tool
    }

    /// Asks for permission with `tool` from the next start of the helper on; a helper already up
    /// keeps running until it is let go.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    /// Repeats `reload` after a change from now on, as the settings last chose it.
    pub fn set_reload(&mut self, reload: Reload) {
        self.reload = reload;
    }

    /// Whether the helper is up, so a transaction runs without asking for the password.
    #[must_use]
    pub fn has_helper(&self) -> bool {
        self.granted
    }

    /// Where the flow is.
    #[must_use]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Whether the output pane is on screen: while pacman runs and after it failed.
    #[must_use]
    pub fn shows_output(&self) -> bool {
        matches!(self.state, State::Running { .. } | State::Finished { .. })
    }

    /// Whether an action is being planned, so the control that asked can show it.
    #[must_use]
    pub fn is_planning(&self) -> bool {
        matches!(self.state, State::Planning(_))
    }

    /// Rows the output pane takes.
    #[must_use]
    pub fn output_height(&self) -> u16 {
        self.output_height
    }

    /// Applies `msg` and says what to do next. `pty` is the terminal pacman gets if it starts
    /// now, in columns and rows: the size of the pane its output fills, so its progress bars end
    /// where the pane does.
    pub fn update(&mut self, msg: Msg, pty: (u16, u16)) -> Command<AppMsg> {
        match msg {
            Msg::Begin(action) => return self.begin(action),
            Msg::Planned(planned) => return self.planned(planned),
            Msg::Cancel => {
                if matches!(self.state, State::Confirming { .. } | State::Locked { .. }) {
                    self.state = State::Idle;
                }
            }
            Msg::Apply => return self.apply(pty),
            Msg::Warmed(outcome) => return self.warmed(outcome),
            Msg::HelperStarted(outcome) => return self.helper_started(outcome, pty),
            Msg::HelperLine(start, line) => self.helper_line(start, line),
            Msg::Started(result) => return self.started(result, pty),
            Msg::Line(text) => self.line(&text),
            Msg::Ended(outcome) => return self.ended(&outcome),
            Msg::Refused(refusal) => return self.refused(refusal),
            Msg::Lost(reason) => return self.lost(&reason),
            Msg::Event(event) => return self.event(event),
            Msg::EndHelper => {
                // A running transaction holds the helper; it is let go only between them.
                if !matches!(self.state, State::Running { .. } | State::Starting(_)) {
                    self.session.end();
                    self.granted = false;
                }
            }
            Msg::Stop => {
                if let State::Running { task, .. } = &self.state {
                    return Command::cancel_task(*task);
                }
            }
            Msg::Close => {
                if matches!(self.state, State::Finished { .. }) {
                    self.state = State::Idle;
                }
            }
            Msg::OutputHeight(rows) => self.output_height = rows,
        }
        Command::none()
    }

    /// Starts planning `action` in the background, unless something is already going on.
    fn begin(&mut self, action: Action) -> Command<AppMsg> {
        if !matches!(self.state, State::Idle | State::Finished { .. }) || action.names().is_empty() {
            return Command::none();
        }
        self.state = State::Planning(action.clone());
        let runner = Arc::clone(&self.runner);
        let session = self.session.clone();
        let lock_dir = self.lock_dir.clone();
        Command::perform(move || wrap(Msg::Planned(plan(runner.as_ref(), &session, &lock_dir, action))))
    }

    /// Shows the plan, the lock notice, or why there is no plan.
    fn planned(&mut self, planned: Planned) -> Command<AppMsg> {
        if !matches!(&self.state, State::Planning(action) if *action == planned.action) {
            return Command::none();
        }
        self.granted = planned.granted;
        if let LockStatus::Held { since, owner } = planned.lock {
            self.state = State::Locked { since, owner };
            return Command::none();
        }
        match planned.plan {
            Ok(plan) => {
                self.state = State::Confirming { action: planned.action, plan, granted: planned.granted };
                Command::none()
            }
            Err(reason) => {
                self.state = State::Idle;
                self.toast(Toast::danger(t!("transaction.plan-failed")).body(reason))
            }
        }
    }

    /// Runs the confirmed plan through the helper, starting the helper first when there is none.
    fn apply(&mut self, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Confirming { action, plan, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = self.session.is_alive();
        if self.granted {
            return self.execute(action, pty);
        }
        if let Tool::Pkexec(program) = &self.tool {
            let program = program.clone();
            return self.ask_polkit(program, action, plan.steps.len());
        }
        self.state = State::Authorizing(action);
        let handoff = Handoff::new(SUDO, |outcome| wrap(Msg::Warmed(outcome)))
            .args(command::warm_ticket())
            .notice(t!("transaction.authorizing"));
        Command::handoff(handoff)
    }

    /// Starts the helper once sudo said yes; otherwise nothing runs and the list stays.
    fn warmed(&mut self, outcome: HandoffOutcome) -> Command<AppMsg> {
        let State::Authorizing(action) = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        match outcome {
            HandoffOutcome::Finished { code: Some(0) } => {
                self.state = State::Starting(action);
                let session = self.session.clone();
                Command::perform(move || wrap(Msg::Started(session.start())))
            }
            HandoffOutcome::Finished { .. } => self.not_authorized(t!("transaction.not-authorized-body")),
            HandoffOutcome::Failed(reason) => self.not_authorized(reason),
        }
    }

    /// Hands the terminal over to pkexec, which asks for the password and starts the helper at
    /// qpac's own absolute path. qpac's three lines come first, so the user knows who asks and
    /// why before polkit's own text; the screen comes back once the helper says it is ready.
    fn ask_polkit(&mut self, program: PathBuf, action: Action, packages: usize) -> Command<AppMsg> {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => return self.helper_failed(error.to_string()),
        };
        self.starts += 1;
        let start = self.starts;
        let asks = if action.is_removal() {
            t!("transaction.polkit-asks-remove", n = packages)
        } else {
            t!("transaction.polkit-asks-install", n = packages)
        };
        // The empty last line sets qpac's words apart from pkexec's.
        let notice = format!("{asks}\n{}\n{}\n", t!("transaction.polkit-password"), t!("transaction.polkit-lasts"));
        self.state = State::Authorizing(action);
        let handoff = DetachedHandoff::new(program, |outcome| wrap(Msg::HelperStarted(outcome)))
            .args(pkexec::args(&exe, session::locale().as_deref()))
            .notice(notice)
            .on_line(move |line| wrap(Msg::HelperLine(start, line)));
        Command::handoff_detached(handoff)
    }

    /// Takes the helper pkexec started into the session and runs the action through it; when
    /// pkexec ended without starting one, permission was not given and nothing ran.
    fn helper_started(&mut self, outcome: DetachedOutcome, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Authorizing(action) = std::mem::replace(&mut self.state, State::Idle) else {
            // Nothing waits for this helper; it is let go at once.
            if let DetachedOutcome::Detached { child, .. } = outcome {
                child.close_stdin();
            }
            return Command::none();
        };
        match outcome {
            DetachedOutcome::Detached { child, first_line } => {
                let (connection, lines) = pkexec::connect(child);
                self.helper_lines = Some((self.starts, lines));
                self.state = State::Starting(action);
                let result = self.session.adopt(connection, &first_line);
                self.started(result, pty)
            }
            // pkexec ends with 126 when the password was refused or the prompt dismissed, and
            // 127 when it could not authenticate at all; a helper that ended before saying it is
            // ready never had the chance to change anything either.
            DetachedOutcome::Finished { .. } => self.not_authorized(t!("transaction.not-authorized-body")),
            DetachedOutcome::Failed(reason) => self.helper_failed(reason),
        }
    }

    /// Hands a line of the helper pkexec started to the session, or tells it the helper ended.
    fn helper_line(&mut self, start: u64, line: ChildLine) {
        let Some((current, lines)) = &self.helper_lines else {
            return;
        };
        if *current != start {
            return;
        }
        match line {
            ChildLine::Line(text) => {
                // The receiver is gone only when the session let this helper go.
                let _ = lines.send(Ok(text));
            }
            ChildLine::Ended { .. } => {
                self.helper_lines = None;
                // A run in progress hears of the end through its connection; between runs the
                // badge goes as soon as the helper does.
                if !matches!(self.state, State::Running { .. } | State::Starting(_)) {
                    self.granted = self.session.is_alive();
                }
            }
        }
    }

    /// Says the helper could not be started at all; nothing was changed.
    fn helper_failed(&self, reason: String) -> Command<AppMsg> {
        self.toast(
            Toast::danger(t!("transaction.helper-failed"))
                .body(t!("transaction.not-authorized-reason", reason = reason)),
        )
    }

    /// Runs the action once the helper is up; otherwise says why and runs nothing.
    fn started(&mut self, result: Result<(), StartFailure>, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Starting(action) = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = result.is_ok();
        let reason = match result {
            Ok(()) => return self.execute(action, pty),
            Err(StartFailure::Refused(refusal)) => refusal_text(refusal),
            Err(StartFailure::Silent) => t!("helper.silent"),
            Err(StartFailure::Failed(text)) if text.is_empty() => t!("helper.ended"),
            Err(StartFailure::Failed(text)) => text,
        };
        self.not_authorized(t!("transaction.not-authorized-reason", reason = reason))
    }

    /// Says the helper could not be had and why; nothing was changed.
    fn not_authorized(&self, body: String) -> Command<AppMsg> {
        self.toast(Toast::danger(t!("transaction.not-authorized")).body(body))
    }

    /// Has the helper run the action on a `pty`-sized pseudo-terminal and streams its output
    /// into the pane.
    ///
    /// pacman's output is in the user's own language: it is shown, never parsed. The
    /// pseudo-terminal keeps pacman's progress bars, fitted to the pane.
    ///
    /// Stopping stops the wait and lets the helper go: pacman runs on to its end, because
    /// stopping it halfway can break its database, and the helper exits after it. The stop
    /// toast says so to the user, and the lock check before the next transaction catches a
    /// pacman still at work.
    fn execute(&mut self, action: Action, pty: (u16, u16)) -> Command<AppMsg> {
        let session = self.session.clone();
        let request = action.request();
        let label = running_label(&action);
        let task = Task::new(label, move |cx| {
            let outcome = session.run(&request, pty, &InTask(cx), &mut |line| cx.send(wrap(Msg::Line(line))));
            Ok(wrap(match outcome {
                Outcome::Finished(outcome) => Msg::Ended(outcome),
                Outcome::Refused(refusal) => Msg::Refused(refusal),
                Outcome::Lost(reason) => Msg::Lost(reason),
            }))
        })
        .on_event(|event| wrap(Msg::Event(event)));
        self.state = State::Running { action, task: task.id(), output: LogBuffer::new(OUTPUT_LINES), progress: None };
        Command::task(task)
    }

    /// Keeps a line pacman printed, without its escape sequences, and reads its step counter.
    fn line(&mut self, text: &str) {
        let State::Running { output, progress, .. } = &mut self.state else {
            return;
        };
        let clean = filter::clean(text);
        if let Some(fraction) = filter::step(&clean) {
            *progress = Some(fraction);
        }
        if !clean.trim().is_empty() {
            output.push(LogLine::new(LogLevel::Info, clean));
        }
    }

    /// Reports how pacman ended and, after a success, reads the packages and the sources again:
    /// what was just installed may be a source's own program.
    fn ended(&mut self, outcome: &ProcessOutcome) -> Command<AppMsg> {
        let State::Running { action, output, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = self.session.is_alive();
        let names = action.names_text();
        match outcome {
            ProcessOutcome::Finished { code: Some(0) } => {
                let title = if action.is_removal() {
                    t!("transaction.done-remove", names = names)
                } else {
                    t!("transaction.done-install", names = names)
                };
                Command::batch([self.toast(Toast::success(title)), self.reload.command()])
            }
            ProcessOutcome::Finished { code: Some(code) } => {
                self.state = State::Finished { action, output };
                self.toast(Toast::danger(t!("transaction.failed", code = *code)).body(t!("transaction.failed-body")))
            }
            ProcessOutcome::Finished { code: None } => {
                self.state = State::Finished { action, output };
                self.toast(Toast::danger(t!("transaction.killed")).body(t!("transaction.failed-body")))
            }
            ProcessOutcome::Cancelled => {
                self.state = State::Finished { action, output };
                self.toast(Toast::info(t!("transaction.cancelled")).body(t!("transaction.cancelled-body")))
            }
        }
    }

    /// Says the helper refused the request. Nothing ran, so there is no output to keep.
    fn refused(&mut self, refusal: Refusal) -> Command<AppMsg> {
        if !matches!(self.state, State::Running { .. }) {
            return Command::none();
        }
        self.state = State::Idle;
        self.toast(Toast::danger(t!("transaction.refused")).body(refusal_text(refusal)))
    }

    /// Says the helper went away mid-run; what pacman said so far stays on screen.
    fn lost(&mut self, reason: &str) -> Command<AppMsg> {
        let State::Running { action, output, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = false;
        self.state = State::Finished { action, output };
        let body = if reason.is_empty() { t!("helper.ended") } else { reason.to_owned() };
        self.toast(Toast::danger(t!("transaction.lost")).body(body))
    }

    /// Notices a task that ended without delivering pacman's outcome: one that was stopped, whose
    /// result the runtime drops, or one that could not start pacman at all.
    fn event(&mut self, event: TaskEvent) -> Command<AppMsg> {
        let TaskEvent::Finished { id, outcome } = event else {
            return Command::none();
        };
        if !matches!(&self.state, State::Running { task, .. } if *task == id) {
            return Command::none();
        }
        match outcome {
            TaskOutcome::Done => Command::none(),
            TaskOutcome::Cancelled => self.ended(&ProcessOutcome::Cancelled),
            TaskOutcome::Failed(reason) => {
                let State::Running { action, output, .. } = std::mem::replace(&mut self.state, State::Idle) else {
                    return Command::none();
                };
                self.granted = self.session.is_alive();
                self.state = State::Finished { action, output };
                self.toast(Toast::danger(t!("transaction.could-not-start")).body(reason))
            }
        }
    }

    /// Shows `toast` in the flow's place, replacing the one before it.
    fn toast(&self, toast: Toast<AppMsg>) -> Command<AppMsg> {
        Command::toast(toast.key(TOAST_KEY))
    }
}

/// A wait for the helper inside the task running pacman: stopping the task gives it up, and a
/// pause is the task's own sleep, which a test's clock decides.
struct InTask<'a>(&'a TaskCx<AppMsg>);

impl Wait for InTask<'_> {
    fn given_up(&self) -> bool {
        self.0.is_cancelled()
    }

    fn pause(&self, duration: Duration) -> bool {
        self.0.sleep(duration)
    }
}

/// The heading of the output pane while `action` runs.
fn running_label(action: &Action) -> String {
    let names = action.names_text();
    if action.is_removal() {
        t!("transaction.running-remove", names = names)
    } else {
        t!("transaction.running-install", names = names)
    }
}

/// What the helper's refusal means, in the user's words.
fn refusal_text(refusal: Refusal) -> String {
    t!(&format!("helper.refused.{}", refusal.key()))
}

/// Finds out, without privileges, what `action` would do, whether the helper is up so no
/// password will be asked, and whether the database is free. Runs in the background.
fn plan(runner: &dyn Runner, session: &Session, lock_dir: &Path, action: Action) -> Planned {
    let env = command::parsed_env();
    let plan = match runner.output(PACMAN, &action.print_args(), &env) {
        Ok(output) if output.succeeded() => Ok(action.parse_plan(&output.stdout)),
        Ok(output) => Err(failure_text(&output.stderr, &output.stdout)),
        Err(error) => Err(error.to_string()),
    };
    Planned { action, plan, granted: session.is_alive(), lock: lock_status(lock_dir) }
}

/// What pacman said when it refused: its error stream, or its output when the error stream is
/// empty, trimmed to the lines that carry words.
fn failure_text(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    text.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_remove_build_their_plans_and_requests() {
        let install = Action::Install(vec!["paru".to_owned()]);
        assert_eq!(install.print_args(), ["-S", "--print", "--print-format", "%r|%n|%v|%s", "--", "paru"]);
        assert_eq!(install.request(), Request::Install(vec!["paru".to_owned()]));
        let remove = Action::Remove(vec!["yay".to_owned(), "bash".to_owned()]);
        assert_eq!(remove.print_args(), ["-Rns", "--print", "--print-format", "%n|%v", "--", "yay", "bash"]);
        assert_eq!(remove.request().to_string(), "remove yay bash");
        assert!(remove.is_removal());
        assert_eq!(remove.names_text(), "yay, bash");
    }

    #[test]
    fn a_refusal_reads_from_the_error_stream_first() {
        assert_eq!(failure_text("error: target not found: nope\n", "ignored"), "error: target not found: nope");
        assert_eq!(failure_text("  \n", "  line one\n\nline two  \n"), "line one line two");
    }
}

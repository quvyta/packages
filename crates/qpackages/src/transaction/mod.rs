//! A pacman transaction from the user's decision to its result: plan without privileges, confirm
//! on our screen, have the root helper run pacman on a pseudo-terminal while the list stays where
//! it is, then say how it went.
//!
//! The first transaction of a run starts the helper, which stays up until qpac quits. Where polkit
//! is installed, pkexec asks for the password while the terminal is handed over and starts the
//! helper itself; elsewhere sudo asks on the real terminal, then `sudo -n` starts it. Later
//! transactions go straight to it. No password ever passes through this code.

pub mod filter;
mod job;
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
use qframe::widgets::Toast;
use qpackages_core::backup;
use qpackages_core::helper::{Refusal, Request};
use qpackages_core::lock::{LockStatus, Owner, lock_status};
use qpackages_core::pacman::command::{self, PACMAN, SUDO};
use qpackages_core::pacman::{Plan, Update, parse_install_plan, parse_remove_plan, read_orphans};
use qpackages_core::reflector::Mirrors;

use crate::app::Msg as AppMsg;
use crate::helper::pkexec::{self, Tool};
use crate::helper::session::{self, Outcome, Session, StartFailure, Wait};
use crate::reload::Reload;
use crate::runner::Runner;
pub use job::{Job, Step};

/// Lines of output kept; the oldest fall out once pacman has said more than this.
const OUTPUT_LINES: usize = 50_000;

/// The key every toast of the flow shares, so a later one replaces the earlier in place.
const TOAST_KEY: &str = "transaction";

/// What the user asked the helper to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Install these packages from the repositories.
    Install(Vec<String>),
    /// Remove these packages with the dependencies only they needed and their saved settings.
    Remove(Vec<String>),
    /// Bring the whole system up to date; these are the updates the check found, which the
    /// confirmation lists.
    Upgrade(Vec<Update>),
    /// Bring the whole system up to date and install these packages in the same transaction.
    UpgradeInstall(Vec<String>),
    /// Remove these orphans; the helper removes them only if they are still exactly the orphans.
    RemoveOrphans(Vec<String>),
    /// Choose pacman's mirrors with reflector.
    Mirrors(Mirrors),
    /// Turn reflector's timer on or off.
    Timer(bool),
}

impl Action {
    /// The packages named; none for the mirror settings.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Install(names) | Self::Remove(names) | Self::UpgradeInstall(names) | Self::RemoveOrphans(names) => {
                names.iter().map(String::as_str).collect()
            }
            Self::Upgrade(updates) => updates.iter().map(|update| update.name.as_str()).collect(),
            Self::Mirrors(_) | Self::Timer(_) => Vec::new(),
        }
    }

    /// Whether there is nothing to do: a package action without packages.
    fn is_empty(&self) -> bool {
        !matches!(self, Self::Mirrors(_) | Self::Timer(_)) && self.names().is_empty()
    }

    /// Whether the action takes packages away.
    #[must_use]
    pub fn is_removal(&self) -> bool {
        matches!(self, Self::Remove(_) | Self::RemoveOrphans(_))
    }

    /// Whether the action brings the system up to date, which the snapshots are taken around.
    #[must_use]
    pub fn upgrades(&self) -> bool {
        matches!(self, Self::Upgrade(_) | Self::UpgradeInstall(_))
    }

    /// Whether the action changes the installed packages: those wait for pacman's lock, and the
    /// screen reads the packages again after them.
    #[must_use]
    pub fn changes_packages(&self) -> bool {
        !matches!(self, Self::Mirrors(_) | Self::Timer(_))
    }

    /// The names as one line, for toasts and headings.
    #[must_use]
    pub fn names_text(&self) -> String {
        self.names().join(", ")
    }

    /// The pacman arguments that only print the plan; `None` where pacman cannot print one
    /// without privileges (an update needs the refreshed databases) or where no package moves.
    fn print_args(&self) -> Option<Vec<String>> {
        match self {
            Self::Install(names) | Self::UpgradeInstall(names) => Some(command::print_install(names)),
            Self::Remove(names) | Self::RemoveOrphans(names) => Some(command::print_remove(names)),
            Self::Upgrade(_) | Self::Mirrors(_) | Self::Timer(_) => None,
        }
    }

    /// The request that has the helper carry the action out.
    fn request(&self) -> Request {
        match self {
            Self::Install(names) => Request::Install(names.clone()),
            Self::Remove(names) => Request::Remove(names.clone()),
            Self::Upgrade(_) => Request::Upgrade,
            Self::UpgradeInstall(names) => Request::UpgradeInstall(names.clone()),
            Self::RemoveOrphans(names) => Request::RemoveOrphans(names.clone()),
            Self::Mirrors(mirrors) => Request::Mirrors(mirrors.clone()),
            Self::Timer(on) => Request::Timer(*on),
        }
    }

    /// Reads the plan pacman printed for this action.
    fn parse_plan(&self, text: &str) -> Plan {
        if self.is_removal() { parse_remove_plan(text) } else { parse_install_plan(text) }
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

/// What the application does once the packages were read again after a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done {
    /// The system was brought up to date, so the updates are looked for again.
    pub upgraded: bool,
    /// Packages were removed or updated, which can leave orphans behind.
    pub orphans: bool,
}

/// Everything that can happen in the flow.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The user asked for an action; planning starts.
    Begin(Action),
    /// Carries out an action the user already agreed to, without asking again: the orphans the
    /// user chose to have removed on their own.
    BeginConfirmed(Action),
    /// Planning ended.
    Planned(Planned),
    /// The user closed the confirmation or the lock notice.
    Cancel,
    /// The user confirmed the plan.
    Apply,
    /// The user confirmed an installation, to run together with the pending updates.
    ApplyWithUpgrade,
    /// No snapshot could be taken and the user chose to go on without one.
    ContinueWithoutBackup,
    /// No snapshot could be taken and the user chose to stop.
    DropJob,
    /// The user closed the list of new `.pacnew` files.
    ClosePacnew,
    /// The orphans were listed again after the helper said the list had changed.
    Orphans(Result<Vec<String>, String>),
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
    /// The running step printed a line.
    Line(String),
    /// The running step ended.
    Ended(ProcessOutcome),
    /// The helper refused the request; nothing ran.
    Refused(Refusal),
    /// The helper went away before the step's end was known; what was known about it.
    Lost(String),
    /// The user let the helper go.
    EndHelper,
    /// The task running a step started, progressed or ended.
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
        /// What pacman would do; empty where it cannot say without privileges.
        plan: Plan,
        /// Whether the helper is up, so no password will be asked.
        granted: bool,
        /// What happens around an update.
        backup: backup::Plan,
        /// How many updates wait, which an installation offers to bring along.
        pending: usize,
    },
    /// Another transaction holds pacman's database; nothing can start.
    Locked {
        /// When the lock was taken, when the file said.
        since: Option<SystemTime>,
        /// The process holding it, when it could be seen.
        owner: Option<Owner>,
    },
    /// No snapshot could be taken; the user is asked whether to go on without one.
    BackupFailed(Job),
    /// sudo or pkexec has the terminal and is asking for the password.
    Authorizing(Job),
    /// The helper is starting.
    Starting(Job),
    /// A step is running.
    Running {
        /// The job.
        job: Job,
        /// The task streaming the step's output.
        task: TaskId,
    },
    /// A step ended without success; the output stays on screen until closed.
    Finished(Job),
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
    /// What happens around a system update, as the settings and the machine decide.
    backup: backup::Plan,
    /// How many repository updates wait, as the last check found.
    pending_updates: usize,
    /// The `.pacnew` files the last transaction left, until the user closes their list.
    pacnew: Option<Vec<String>>,
    /// What to do once the packages were read again after the last transaction.
    done: Option<Done>,
}

impl fmt::Debug for Flow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flow")
            .field("reload", &self.reload)
            .field("lock_dir", &self.lock_dir)
            .field("state", &self.state)
            .field("output_height", &self.output_height)
            .field("backup", &self.backup)
            .field("pending_updates", &self.pending_updates)
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
            backup: backup::Plan::Off,
            pending_updates: 0,
            pacnew: None,
            done: None,
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

    /// Takes snapshots around the next system update as `plan` says.
    pub fn set_backup(&mut self, plan: backup::Plan) {
        self.backup = plan;
    }

    /// `count` repository updates wait: an installation confirmed from now on offers to bring
    /// them along.
    pub fn set_pending_updates(&mut self, count: usize) {
        self.pending_updates = count;
    }

    /// What to do after the read that followed the last transaction; asked once.
    pub fn take_done(&mut self) -> Option<Done> {
        self.done.take()
    }

    /// The `.pacnew` files the last transaction left, while their list is open.
    #[must_use]
    pub fn pacnew(&self) -> Option<&[String]> {
        self.pacnew.as_deref()
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

    /// Whether nothing is going on, so a new action can start.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle | State::Finished(_))
    }

    /// Whether the output pane is on screen: while a step runs, after one failed, and while the
    /// user is asked about a snapshot that failed.
    #[must_use]
    pub fn shows_output(&self) -> bool {
        match &self.state {
            State::Running { .. } | State::Finished(_) => true,
            State::BackupFailed(job) => !job.output.is_empty(),
            _ => false,
        }
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
            Msg::BeginConfirmed(action) => {
                if self.is_idle() && !action.is_empty() {
                    self.state = State::Idle;
                    return self.start_job(Job::new(action, self.backup), pty);
                }
            }
            Msg::Planned(planned) => return self.planned(planned),
            Msg::Cancel => {
                if matches!(self.state, State::Confirming { .. } | State::Locked { .. }) {
                    self.state = State::Idle;
                }
            }
            Msg::Apply => return self.apply(false, pty),
            Msg::ApplyWithUpgrade => return self.apply(true, pty),
            Msg::ContinueWithoutBackup => {
                if let State::BackupFailed(mut job) = std::mem::replace(&mut self.state, State::Idle) {
                    job.without_snapshots();
                    return self.start_job(job, pty);
                }
            }
            Msg::DropJob => {
                if matches!(self.state, State::BackupFailed(_)) {
                    self.state = State::Idle;
                    return self.toast(Toast::info(t!("backup.stopped")).body(t!("transaction.not-authorized-body")));
                }
            }
            Msg::ClosePacnew => self.pacnew = None,
            Msg::Orphans(found) => return self.orphans_again(found),
            Msg::Warmed(outcome) => return self.warmed(outcome),
            Msg::HelperStarted(outcome) => return self.helper_started(outcome, pty),
            Msg::HelperLine(start, line) => self.helper_line(start, line),
            Msg::Started(result) => return self.started(result, pty),
            Msg::Line(text) => {
                if let State::Running { job, .. } = &mut self.state {
                    job.line(&text);
                }
            }
            Msg::Ended(outcome) => return self.ended(&outcome, pty),
            Msg::Refused(refusal) => return self.refused(refusal),
            Msg::Lost(reason) => return self.lost(&reason),
            Msg::Event(event) => return self.event(event, pty),
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
                if matches!(self.state, State::Finished(_)) {
                    self.state = State::Idle;
                }
            }
            Msg::OutputHeight(rows) => self.output_height = rows,
        }
        Command::none()
    }

    /// Starts planning `action` in the background, unless something is already going on.
    fn begin(&mut self, action: Action) -> Command<AppMsg> {
        if !self.is_idle() || action.is_empty() {
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
                // Only an installation offers to bring the updates along; the others either are
                // an update already or touch no package that a partial update could break.
                let pending = if matches!(planned.action, Action::Install(_)) { self.pending_updates } else { 0 };
                self.state = State::Confirming {
                    action: planned.action,
                    plan,
                    granted: planned.granted,
                    backup: self.backup,
                    pending,
                };
                Command::none()
            }
            Err(reason) => {
                self.state = State::Idle;
                self.toast(Toast::danger(t!("transaction.plan-failed")).body(reason))
            }
        }
    }

    /// Carries out the confirmed plan, with the pending updates when `with_upgrade`. A snapshot
    /// tool that is chosen but missing is said before anything is asked or run.
    fn apply(&mut self, with_upgrade: bool, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Confirming { action, backup, plan, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        let action = match action {
            Action::Install(names) if with_upgrade => Action::UpgradeInstall(names),
            action => action,
        };
        let mut job = Job::new(action, backup);
        if !plan.steps.is_empty() {
            job.packages = plan.steps.len();
        }
        if let backup::Plan::Unavailable(tool) = backup
            && job.action.upgrades()
        {
            let reason = t!("backup.not-installed", tool = tool.key());
            return self.ask_without_backup(job, reason);
        }
        self.start_job(job, pty)
    }

    /// Runs `job` through the helper, starting the helper first when there is none.
    fn start_job(&mut self, job: Job, pty: (u16, u16)) -> Command<AppMsg> {
        self.granted = self.session.is_alive();
        if self.granted {
            return self.execute(job, pty);
        }
        if let Tool::Pkexec(program) = &self.tool {
            let program = program.clone();
            return self.ask_polkit(program, job);
        }
        self.state = State::Authorizing(job);
        let handoff = Handoff::new(SUDO, |outcome| wrap(Msg::Warmed(outcome)))
            .args(command::warm_ticket())
            .notice(t!("transaction.authorizing"));
        Command::handoff(handoff)
    }

    /// Holds `job` and asks whether to go on without the snapshot that could not be taken.
    fn ask_without_backup(&mut self, job: Job, reason: String) -> Command<AppMsg> {
        self.state = State::BackupFailed(job);
        let question = Confirm::new(t!("backup.failed-title"), wrap(Msg::ContinueWithoutBackup))
            .message(t!("backup.failed", reason = reason))
            .confirm_label(t!("backup.continue"))
            .cancel_label(t!("backup.stop"))
            .on_cancel(wrap(Msg::DropJob));
        Command::confirm(question)
    }

    /// Starts the helper once sudo said yes; otherwise nothing runs and the list stays.
    fn warmed(&mut self, outcome: HandoffOutcome) -> Command<AppMsg> {
        let State::Authorizing(job) = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        match outcome {
            HandoffOutcome::Finished { code: Some(0) } => {
                self.state = State::Starting(job);
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
    fn ask_polkit(&mut self, program: PathBuf, job: Job) -> Command<AppMsg> {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => return self.helper_failed(error.to_string()),
        };
        self.starts += 1;
        let start = self.starts;
        let asks = polkit_asks(&job.action, job.packages);
        // The empty last line sets qpac's words apart from pkexec's.
        let notice = format!("{asks}\n{}\n{}\n", t!("transaction.polkit-password"), t!("transaction.polkit-lasts"));
        self.state = State::Authorizing(job);
        let handoff = DetachedHandoff::new(program, |outcome| wrap(Msg::HelperStarted(outcome)))
            .args(pkexec::args(&exe, session::locale().as_deref()))
            .notice(notice)
            .on_line(move |line| wrap(Msg::HelperLine(start, line)));
        Command::handoff_detached(handoff)
    }

    /// Takes the helper pkexec started into the session and runs the job through it; when
    /// pkexec ended without starting one, permission was not given and nothing ran.
    fn helper_started(&mut self, outcome: DetachedOutcome, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Authorizing(job) = std::mem::replace(&mut self.state, State::Idle) else {
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
                self.state = State::Starting(job);
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

    /// Runs the job once the helper is up; otherwise says why and runs nothing.
    fn started(&mut self, result: Result<(), StartFailure>, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Starting(job) = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = result.is_ok();
        let reason = match result {
            Ok(()) => return self.execute(job, pty),
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

    /// Has the helper run the job's current step on a `pty`-sized pseudo-terminal and streams
    /// its output into the pane.
    ///
    /// pacman's output is in the user's own language: it is shown, never parsed, except for the
    /// paths of new `.pacnew` files and snapper's snapshot number, which read the same in every
    /// language. The pseudo-terminal keeps pacman's progress bars, fitted to the pane.
    ///
    /// Stopping stops the wait and lets the helper go: pacman runs on to its end, because
    /// stopping it halfway can break its database, and the helper exits after it. The stop
    /// toast says so to the user, and the lock check before the next transaction catches a
    /// pacman still at work.
    fn execute(&mut self, job: Job, pty: (u16, u16)) -> Command<AppMsg> {
        let Some(request) = job.request() else {
            return self.succeeded(&job, None);
        };
        let session = self.session.clone();
        let task = Task::new(job.heading(), move |cx| {
            let outcome = session.run(&request, pty, &InTask(cx), &mut |line| cx.send(wrap(Msg::Line(line))));
            Ok(wrap(match outcome {
                Outcome::Finished(outcome) => Msg::Ended(outcome),
                Outcome::Refused(refusal) => Msg::Refused(refusal),
                Outcome::Lost(reason) => Msg::Lost(reason),
            }))
        })
        .on_event(|event| wrap(Msg::Event(event)));
        self.state = State::Running { job, task: task.id() };
        Command::task(task)
    }

    /// Goes on after a step ended: to the next step, to the question about a snapshot that
    /// failed, or to the report of how the job went.
    fn ended(&mut self, outcome: &ProcessOutcome, pty: (u16, u16)) -> Command<AppMsg> {
        let State::Running { mut job, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = self.session.is_alive();
        let success = *outcome == ProcessOutcome::Finished { code: Some(0) };
        match job.step() {
            Some(Step::Before(_)) if success && job.took_before() => {
                job.advance();
                self.execute(job, pty)
            }
            Some(Step::Before(_)) if !matches!(outcome, ProcessOutcome::Cancelled) => {
                let reason = match outcome {
                    ProcessOutcome::Finished { code: Some(0) } => t!("backup.no-number"),
                    ProcessOutcome::Finished { code: Some(code) } => t!("backup.code", code = *code),
                    _ => t!("backup.killed"),
                };
                self.ask_without_backup(job, reason)
            }
            Some(Step::Main) if success => {
                if job.advance() {
                    self.execute(job, pty)
                } else {
                    self.succeeded(&job, None)
                }
            }
            Some(Step::After) => {
                // The update itself went through; a snapshot after it that failed is said, not
                // held against it.
                let warning = (!success).then(|| t!("backup.after-failed"));
                self.succeeded(&job, warning)
            }
            _ => self.failed(job, outcome),
        }
    }

    /// Reports a job that went through, keeps its `.pacnew` files for their list and reads the
    /// packages again: what was just installed may be a source's own program. `warning` is
    /// added when something beside the job did not work.
    fn succeeded(&mut self, job: &Job, warning: Option<String>) -> Command<AppMsg> {
        self.state = State::Idle;
        if !job.pacnew.is_empty() {
            self.pacnew = Some(job.pacnew.clone());
        }
        let names = job.action.names_text();
        let toast = match &job.action {
            Action::Install(_) => Toast::success(t!("transaction.done-install", names = names)),
            Action::Remove(_) => Toast::success(t!("transaction.done-remove", names = names)),
            Action::Upgrade(_) => Toast::success(t!("transaction.done-upgrade")),
            Action::UpgradeInstall(_) => Toast::success(t!("transaction.done-upgrade-install", names = names)),
            Action::RemoveOrphans(list) => Toast::success(t!("transaction.done-orphans", n = list.len())).body(names),
            Action::Mirrors(_) => Toast::success(t!("transaction.done-mirrors")),
            Action::Timer(true) => Toast::success(t!("transaction.done-timer-on")),
            Action::Timer(false) => Toast::success(t!("transaction.done-timer-off")),
        };
        let toast = match warning {
            Some(warning) => Toast::warning(warning).body(t!("backup.after-failed-body")),
            None => toast,
        };
        if !job.action.changes_packages() {
            return self.toast(toast);
        }
        self.done = Some(Done {
            upgraded: job.action.upgrades(),
            orphans: matches!(job.action, Action::Remove(_) | Action::Upgrade(_) | Action::UpgradeInstall(_)),
        });
        Command::batch([self.toast(toast), self.reload.command()])
    }

    /// Keeps the output of a step that did not go through on screen and says how it ended.
    fn failed(&mut self, job: Job, outcome: &ProcessOutcome) -> Command<AppMsg> {
        if !job.pacnew.is_empty() {
            self.pacnew = Some(job.pacnew.clone());
        }
        self.state = State::Finished(job);
        match outcome {
            ProcessOutcome::Finished { code: Some(code) } => {
                self.toast(Toast::danger(t!("transaction.failed", code = *code)).body(t!("transaction.failed-body")))
            }
            ProcessOutcome::Finished { code: None } => {
                self.toast(Toast::danger(t!("transaction.killed")).body(t!("transaction.failed-body")))
            }
            ProcessOutcome::Cancelled => {
                self.toast(Toast::info(t!("transaction.cancelled")).body(t!("transaction.cancelled-body")))
            }
        }
    }

    /// Says the helper refused the request. Nothing ran in this step, so a snapshot refused is a
    /// snapshot that failed, and a changed list of orphans is read again and shown anew.
    fn refused(&mut self, refusal: Refusal) -> Command<AppMsg> {
        let State::Running { job, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        if matches!(job.step(), Some(Step::Before(_))) {
            return self.ask_without_backup(job, refusal_text(refusal));
        }
        let toast = self.toast(Toast::danger(t!("transaction.refused")).body(refusal_text(refusal)));
        if refusal == Refusal::Changed && matches!(job.action, Action::RemoveOrphans(_)) {
            let runner = Arc::clone(&self.runner);
            let list = Command::perform(move || wrap(Msg::Orphans(list_orphans(runner.as_ref()))));
            return Command::batch([toast, list]);
        }
        toast
    }

    /// Shows the orphans anew after the helper said they changed: the confirmation again with
    /// the fresh list, or nothing when there are none left.
    fn orphans_again(&mut self, found: Result<Vec<String>, String>) -> Command<AppMsg> {
        match found {
            Ok(names) if !names.is_empty() => self.begin(Action::RemoveOrphans(names)),
            Ok(_) => self.toast(Toast::info(t!("orphans.none-left"))),
            Err(reason) => self.toast(Toast::danger(t!("orphans.unreadable")).body(reason)),
        }
    }

    /// Says the helper went away mid-run; what was said so far stays on screen.
    fn lost(&mut self, reason: &str) -> Command<AppMsg> {
        let State::Running { job, .. } = std::mem::replace(&mut self.state, State::Idle) else {
            return Command::none();
        };
        self.granted = false;
        self.state = State::Finished(job);
        let body = if reason.is_empty() { t!("helper.ended") } else { reason.to_owned() };
        self.toast(Toast::danger(t!("transaction.lost")).body(body))
    }

    /// Notices a task that ended without delivering the step's outcome: one that was stopped,
    /// whose result the runtime drops, or one that could not start at all.
    fn event(&mut self, event: TaskEvent, pty: (u16, u16)) -> Command<AppMsg> {
        let TaskEvent::Finished { id, outcome } = event else {
            return Command::none();
        };
        if !matches!(&self.state, State::Running { task, .. } if *task == id) {
            return Command::none();
        }
        match outcome {
            TaskOutcome::Done => Command::none(),
            TaskOutcome::Cancelled => self.ended(&ProcessOutcome::Cancelled, pty),
            TaskOutcome::Failed(reason) => {
                let State::Running { job, .. } = std::mem::replace(&mut self.state, State::Idle) else {
                    return Command::none();
                };
                self.granted = self.session.is_alive();
                self.state = State::Finished(job);
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
    match action {
        Action::Install(_) => t!("transaction.running-install", names = names),
        Action::Remove(_) => t!("transaction.running-remove", names = names),
        Action::Upgrade(_) => t!("transaction.running-upgrade"),
        Action::UpgradeInstall(_) => t!("transaction.running-upgrade-install", names = names),
        Action::RemoveOrphans(_) => t!("transaction.running-orphans"),
        Action::Mirrors(_) => t!("transaction.running-mirrors"),
        Action::Timer(true) => t!("transaction.running-timer-on"),
        Action::Timer(false) => t!("transaction.running-timer-off"),
    }
}

/// qpac's first line on the terminal pkexec is handed: who asks and for what.
fn polkit_asks(action: &Action, n: usize) -> String {
    match action {
        Action::Install(_) => t!("transaction.polkit-asks-install", n = n),
        Action::Remove(_) | Action::RemoveOrphans(_) => t!("transaction.polkit-asks-remove", n = n),
        Action::Upgrade(_) | Action::UpgradeInstall(_) => t!("transaction.polkit-asks-upgrade"),
        Action::Mirrors(_) => t!("transaction.polkit-asks-mirrors"),
        Action::Timer(_) => t!("transaction.polkit-asks-timer"),
    }
}

/// What the helper's refusal means, in the user's words.
fn refusal_text(refusal: Refusal) -> String {
    t!(&format!("helper.refused.{}", refusal.key()))
}

/// Finds out, without privileges, what `action` would do, whether the helper is up so no
/// password will be asked, and whether the database is free. Runs in the background.
///
/// An update has no printed plan: pacman needs the freshly refreshed databases for it, which
/// only root may write. Its confirmation lists what the update check found instead.
fn plan(runner: &dyn Runner, session: &Session, lock_dir: &Path, action: Action) -> Planned {
    let env = command::parsed_env();
    let plan = match action.print_args() {
        None => Ok(Plan::default()),
        Some(args) => match runner.output(PACMAN, &args, &env) {
            Ok(output) if output.succeeded() => Ok(action.parse_plan(&output.stdout)),
            Ok(output) => Err(failure_text(&output.stderr, &output.stdout)),
            Err(error) => Err(error.to_string()),
        },
    };
    let lock = if action.changes_packages() { lock_status(lock_dir) } else { LockStatus::Free };
    Planned { action, plan, granted: session.is_alive(), lock }
}

/// The orphans as pacman lists them now, without privileges.
///
/// # Errors
///
/// Returns what pacman said when it could not list them.
pub fn list_orphans(runner: &dyn Runner) -> Result<Vec<String>, String> {
    let output = runner.output(PACMAN, &command::orphans(), &command::parsed_env()).map_err(|e| e.to_string())?;
    read_orphans(output.code, &output.stdout, &output.stderr).map_err(|stderr| failure_text(&stderr, ""))
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
        assert_eq!(
            install.print_args().expect("a printed plan"),
            ["-S", "--print", "--print-format", "%r|%n|%v|%s", "--", "paru"]
        );
        assert_eq!(install.request(), Request::Install(vec!["paru".to_owned()]));
        let remove = Action::Remove(vec!["yay".to_owned(), "bash".to_owned()]);
        assert_eq!(
            remove.print_args().expect("a printed plan"),
            ["-Rns", "--print", "--print-format", "%n|%v", "--", "yay", "bash"]
        );
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

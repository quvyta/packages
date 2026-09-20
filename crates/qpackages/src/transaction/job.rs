//! One confirmed transaction as the helper carries it out: its steps in order, what they said,
//! and where it has got to.
//!
//! Most transactions are one step: the request itself. A system update is wrapped in snapshots
//! when the user chose a snapshot tool: one before, the update, and with snapper one after that
//! is paired with the first. Each step is its own request to the helper, so the screen can say
//! which one is running and the snapshot's number can be read before the step that needs it.
//! Removing Flatpak applications for the user is two steps as well: the applications, then the
//! runtimes no application uses any more.

use qframe::prelude::*;
use qframe::widgets::{LogBuffer, LogLevel, LogLine};
use qpackages_core::backup::{self, Snapshot, Tool};
use qpackages_core::flatpak;
use qpackages_core::helper::Request;
use qpackages_core::pacman::pacnew_files;
use qpackages_core::snap::api;

use super::{Action, OUTPUT_LINES, Run, filter};

/// One request of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The snapshot before the update, with this tool.
    Before(Tool),
    /// The action's own request.
    Main,
    /// snapper's snapshot after the update, paired with the one before.
    After,
    /// The user's Flatpak runtimes no application uses any more, after a removal.
    Unused,
}

/// A transaction on its way through the helper.
#[derive(Debug)]
pub struct Job {
    /// What the user confirmed.
    pub action: Action,
    /// Every step, in order.
    steps: Vec<Step>,
    /// The step running now, or the next one to run.
    index: usize,
    /// What every step said so far, without escape sequences.
    pub output: LogBuffer,
    /// The running step's share done, once pacman printed a step counter.
    pub progress: Option<f32>,
    /// The lines of the running step, kept only while a snapshot is taken: snapper prints the
    /// snapshot's number among them.
    step_lines: Vec<String>,
    /// The number snapper gave the snapshot before, which the one after is paired with.
    pre: Option<u32>,
    /// The summary of the snap step last put in the pane, so reading snapd again does not write
    /// the same line over and over.
    snap_step: String,
    /// The `.pacnew` files pacman said it left, in the order it said so.
    pub pacnew: Vec<String>,
    /// How many packages the confirmed plan touches, dependencies included; the names alone
    /// where there was no printed plan.
    pub packages: usize,
    /// The actions that follow once this job went through, each confirmed on its own.
    pub then: Vec<Action>,
    /// What an AUR build builds and installs, as the user confirmed it.
    pub build: Option<qpackages_core::build::Plan>,
}

impl Job {
    /// The job for `action` under the snapshot plan `backup`: snapshots only around a system
    /// update, and only when qpac takes them itself.
    #[must_use]
    pub fn new(action: Action, backup: backup::Plan) -> Self {
        let steps = match backup {
            backup::Plan::Take(tool) if action.upgrades() => match tool {
                Tool::Snapper => vec![Step::Before(tool), Step::Main, Step::After],
                Tool::Timeshift => vec![Step::Before(tool), Step::Main],
            },
            _ if matches!(action, Action::FlatpakRemove(_)) => vec![Step::Main, Step::Unused],
            _ => vec![Step::Main],
        };
        let packages = action.names().len();
        Self {
            action,
            packages,
            steps,
            index: 0,
            output: LogBuffer::new(OUTPUT_LINES),
            progress: None,
            step_lines: Vec::new(),
            snap_step: String::new(),
            pre: None,
            pacnew: Vec::new(),
            then: Vec::new(),
            build: None,
        }
    }

    /// The step running now, or next.
    #[must_use]
    pub fn step(&self) -> Option<Step> {
        self.steps.get(self.index).copied()
    }

    /// How the current step is carried out; `None` once every step ran.
    #[must_use]
    pub fn run(&self) -> Option<Run> {
        match self.step()? {
            Step::Before(Tool::Snapper) => Some(Run::Helper(Request::Snapshot(Snapshot::SnapperPre))),
            Step::Before(Tool::Timeshift) => Some(Run::Helper(Request::Snapshot(Snapshot::Timeshift))),
            Step::Main => Some(self.action.run()),
            Step::After => self.pre.map(|number| Run::Helper(Request::Snapshot(Snapshot::SnapperPost(number)))),
            Step::Unused => Some(Run::Flatpak(flatpak::uninstall_unused_args())),
        }
    }

    /// Keeps a line the current step printed and reads its step counter and `.pacnew` files.
    pub fn line(&mut self, text: &str) {
        let clean = filter::clean(text);
        if let Some(fraction) = filter::step(&clean) {
            self.progress = Some(fraction);
        }
        for file in pacnew_files([clean.as_str()]) {
            if !self.pacnew.contains(&file) {
                self.pacnew.push(file);
            }
        }
        if matches!(self.step(), Some(Step::Before(_))) {
            self.step_lines.push(clean.clone());
        }
        if !clean.trim().is_empty() {
            self.output.push(LogLine::new(LogLevel::Info, clean));
        }
    }

    /// Takes what snapd says about the running snap job: the step it is on goes to the pane the
    /// first time it is seen, and a download's real share drives the bar.
    ///
    /// Only a download counts bytes; every other step counts one of one, and there the bar is left
    /// unknown rather than jumping to a number that means nothing.
    pub fn snap_progress(&mut self, change: &api::Change) {
        let Some(task) = change.running() else { return };
        #[expect(clippy::cast_possible_truncation, reason = "a share between 0 and 1, to draw a bar with")]
        let share = task.fraction().map(|share| share as f32);
        self.progress = share;
        if self.snap_step != task.summary {
            self.snap_step = task.summary.clone();
            self.line(&task.summary);
        }
    }

    /// The snapshot before ended well: its number is kept for the one after. `false` when
    /// snapper printed no number, so no snapshot after can be paired with it.
    pub fn took_before(&mut self) -> bool {
        let number = backup::snapshot_number(self.step_lines.iter().map(String::as_str));
        let snapper = matches!(self.step(), Some(Step::Before(Tool::Snapper)));
        self.step_lines.clear();
        if snapper {
            self.pre = number;
            return number.is_some();
        }
        true
    }

    /// Goes on to the next step. Returns `false` once there is none.
    pub fn advance(&mut self) -> bool {
        self.index += 1;
        self.progress = None;
        self.step_lines.clear();
        self.snap_step.clear();
        self.index < self.steps.len()
    }

    /// Leaves the snapshots out: the user chose to go on without them. The job goes on at the
    /// action's own request.
    pub fn without_snapshots(&mut self) {
        // Only a snapshot before can have failed, so the request has not run yet.
        self.steps.retain(|step| *step == Step::Main);
        self.index = 0;
        self.step_lines.clear();
        self.pre = None;
    }

    /// The heading of the output pane while the job runs: the action alone for a single step,
    /// otherwise the action, the step's place and what it does, as
    /// "Updating the system · 2/3 · installing packages".
    #[must_use]
    pub fn heading(&self) -> String {
        let running = super::running_label(&self.action);
        let Some(step) = self.step().filter(|_| self.steps.len() > 1) else {
            return running;
        };
        let what = match step {
            Step::Before(tool) => t!("transaction.step-before", tool = tool.key()),
            Step::Main if matches!(self.action, Action::FlatpakRemove(_)) => t!("transaction.step-flatpak-apps"),
            Step::Main => t!("transaction.step-main"),
            Step::After => t!("transaction.step-after"),
            Step::Unused => t!("transaction.step-unused"),
        };
        format!("{running} · {}/{} · {what}", self.index + 1, self.steps.len())
    }
}

#[cfg(test)]
mod tests {
    use qpackages_core::pacman::Update;

    use super::*;

    fn upgrade() -> Action {
        Action::Upgrade(vec![Update {
            name: "linux".to_owned(),
            from: "6.18.1-1".to_owned(),
            to: "6.18.2-1".to_owned(),
            ignored: false,
        }])
    }

    /// The request line the current step sends the helper, if it goes to the helper.
    fn helper_line(job: &Job) -> Option<String> {
        match job.run()? {
            Run::Helper(request) => Some(request.to_string()),
            Run::Flatpak(_) | Run::Aur => None,
        }
    }

    #[test]
    fn a_flatpak_removal_for_the_user_clears_the_unused_runtimes_after_it() {
        let ids = vec!["org.gimp.GIMP".to_owned()];
        let mut job = Job::new(Action::FlatpakRemove(ids.clone()), backup::Plan::Take(Tool::Snapper));
        assert_eq!(job.steps, [Step::Main, Step::Unused], "no snapshot, and the runtimes after");
        assert_eq!(job.run(), Some(Run::Flatpak(flatpak::uninstall_args(&ids))));
        assert!(job.advance());
        assert_eq!(job.run(), Some(Run::Flatpak(flatpak::uninstall_unused_args())));
        assert!(!job.advance());
        let system = Job::new(Action::FlatpakRemoveSystem(ids.clone()), backup::Plan::Off);
        assert_eq!(system.steps, [Step::Main]);
        assert_eq!(helper_line(&system).as_deref(), Some("flatpak-system remove org.gimp.GIMP"));
        let install = Job::new(Action::FlatpakInstall(ids.clone()), backup::Plan::Off);
        assert_eq!(install.run(), Some(Run::Flatpak(flatpak::install_args(&ids))));
        let add = Job::new(Action::AddFlathub, backup::Plan::Off);
        assert_eq!(add.run(), Some(Run::Flatpak(flatpak::add_flathub_args())));
    }

    #[test]
    fn snapper_wraps_an_update_in_two_snapshots_paired_by_number() {
        let mut job = Job::new(upgrade(), backup::Plan::Take(Tool::Snapper));
        assert_eq!(helper_line(&job).as_deref(), Some("snapshot pre snapper"));
        job.line("42");
        assert!(job.took_before());
        assert!(job.advance());
        assert_eq!(job.run(), Some(Run::Helper(Request::Upgrade)));
        assert!(job.advance());
        assert_eq!(helper_line(&job).as_deref(), Some("snapshot post snapper 42"));
        assert!(!job.advance());
        assert_eq!(job.run(), None);
    }

    #[test]
    fn timeshift_takes_one_snapshot_and_an_installation_none() {
        let job = Job::new(upgrade(), backup::Plan::Take(Tool::Timeshift));
        assert_eq!(job.steps, [Step::Before(Tool::Timeshift), Step::Main]);
        let install = Job::new(Action::Install(vec!["paru".to_owned()]), backup::Plan::Take(Tool::Snapper));
        assert_eq!(install.steps, [Step::Main], "a plain installation is not wrapped");
        for plan in [backup::Plan::Off, backup::Plan::SnapPac, backup::Plan::Unavailable(Tool::Snapper)] {
            assert_eq!(Job::new(upgrade(), plan).steps, [Step::Main], "{plan:?}");
        }
    }

    #[test]
    fn a_snapper_snapshot_without_a_number_cannot_be_paired() {
        let mut job = Job::new(upgrade(), backup::Plan::Take(Tool::Snapper));
        job.line("Creating snapshot");
        assert!(!job.took_before());
    }

    #[test]
    fn going_on_without_snapshots_leaves_the_request_alone() {
        let mut job = Job::new(upgrade(), backup::Plan::Take(Tool::Snapper));
        job.without_snapshots();
        assert_eq!(job.run(), Some(Run::Helper(Request::Upgrade)));
        assert!(!job.advance(), "nothing after the update either");
    }

    #[test]
    fn pacnew_files_are_gathered_once_from_every_step() {
        let mut job = Job::new(upgrade(), backup::Plan::Off);
        job.line("warning: /etc/pacman.conf installed as /etc/pacman.conf.pacnew");
        job.line("\u{1b}[1mwarning:\u{1b}[0m /etc/pacman.conf installed as /etc/pacman.conf.pacnew");
        job.line("(1/2) upgrading linux");
        assert_eq!(job.pacnew, ["/etc/pacman.conf.pacnew"]);
        assert_eq!(job.progress, Some(0.5));
        assert_eq!(job.output.len(), 3);
    }

    /// What snapd answered about a running job, from the container's recordings.
    fn change(name: &str) -> api::Change {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/snap").join(name);
        let text = std::fs::read_to_string(path).expect("the recording is readable");
        api::parse_change(&text).expect("the recording reads")
    }

    #[test]
    fn a_snap_jobs_step_reaches_the_pane_once_and_a_download_drives_the_bar() {
        let mut job = Job::new(Action::SnapInstall(vec!["core22".to_owned()]), backup::Plan::Off);
        assert_eq!(job.progress, None);
        let doing = change("api-change-doing.json");
        job.snap_progress(&doing);
        let share = job.progress.expect("a download counts bytes");
        assert!((0.006..0.007).contains(&share), "{share}");
        let lines = job.output.len();
        assert!(lines > 0, "snapd's step is in the pane");
        assert!(job.output.iter().any(|line| line.text().contains("Download snap")), "the step it is on");
        job.snap_progress(&doing);
        assert_eq!(job.output.len(), lines, "reading the same step again writes no second line");
    }

    #[test]
    fn a_step_that_only_ends_leaves_the_bar_unknown_and_a_finished_job_changes_nothing() {
        let mut job = Job::new(Action::SnapInstall(vec!["hello".to_owned()]), backup::Plan::Off);
        job.progress = Some(0.5);
        let mut doing = change("api-change-doing.json");
        // The step running now counts one of one, which says nothing about how long it takes.
        for task in &mut doing.tasks {
            if task.is_running() {
                task.total = 1;
                task.done = 0;
            }
        }
        job.snap_progress(&doing);
        assert_eq!(job.progress, None, "a bar that means nothing is left unknown");
        let done = change("api-change-done.json");
        let lines = job.output.len();
        job.snap_progress(&done);
        assert_eq!(job.output.len(), lines, "a job with nothing running says nothing");
    }
}

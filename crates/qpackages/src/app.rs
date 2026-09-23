//! The frame: the application's name, the tabs and the settings button on top, the open tab's
//! page (or the settings page) below, the key hints at the bottom; a transaction's confirmation
//! over it all and its output under the page.
//!
//! Each page is its own module with its own messages and state; the frame owns what they share
//! (the installed packages, the settings, the transaction flow) and does what they ask.

mod backend;
mod layout;
mod self_update;
mod tab;
mod wizard;

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::storage::{self, Family, Settings};
use qframe::widgets::{Appearance, Badge, IconButton, Setup, SetupMsg, Splitter, Tabs, Toast, Tooltip};
use qpackages_core::backup;
use qpackages_core::catalog::net::{CURL, curl_args};
use qpackages_core::news::{NEWS_URL, NewsItem, parse_news, recent};
use qpackages_core::sources::{AurPreference, Availability, Source};

use crate::backend_settings::{self, Orphans};
use crate::helper::pkexec::Tool;
use crate::helper::session::{Session, Start};
use crate::installed::table::Foreign;
use crate::installed::{self, Installed, Library, Show};
use crate::reload::{Lookup, Reload, Snapshot};
use crate::review;
use crate::runner::Runner;
use crate::settings::{self, AUR_HELPERS, PRIVILEGE_TOOLS};
use crate::settings_page::{self, Reflector};
use crate::snap::Snapd;
use crate::transaction::{self, Action, Flow};
use crate::updates::check::Checker;
use crate::updates::{self, Updates};
use crate::{sources, store, transaction::view as flow_view};

use layout::{MIN_PACKAGES, output_layout};
pub use self_update::SelfUpdateFolders;
pub use tab::{TABS, Tab};
pub use wizard::{FirstRun, WizardMsg};

/// The user id of root, for whom the AUR is off limits.
const ROOT: u32 = 0;

/// The machine the application runs on, as its constructor needs it: the real one in `run`, a
/// pretend one in tests.
pub struct Machine<'a> {
    /// Where pacman keeps the records of installed packages.
    pub dbpath: &'a Path,
    /// Where pacman keeps the repository databases; only ever read, to copy them for a check.
    pub sync_dir: &'a Path,
    /// Where the application menu's launchers are, which tell the applications among the
    /// packages.
    pub applications: &'a Path,
    /// The folder qpac keeps its private copy of the repository databases in, when the account
    /// has one.
    pub check_dir: Option<&'a Path>,
    /// The directory pacman's lock file lives in.
    pub lock_dir: &'a Path,
    /// Finds a program on this machine, as `on_path` does: the sources' programs, fakeroot for
    /// the update check, and pkexec for the permission tool.
    pub lookup: Arc<Lookup>,
    /// Runs programs.
    pub runner: Arc<dyn Runner>,
    /// Starts the root helper that carries out transactions when sudo asks for permission;
    /// pkexec starts it through the runtime instead.
    pub helper: Arc<Start>,
    /// The user id the application runs as, or `None` when it could not be read.
    pub uid: Option<u32>,
    /// The machine's distance from UTC in minutes, for the times shown.
    pub utc_offset: i16,
    /// The folder of the repositories' AppStream catalogs, which Discover reads its kinds,
    /// summaries and descriptions from.
    pub app_catalog: &'a Path,
    /// The folders Flatpak keeps its remotes' AppStream catalogs in.
    pub flatpak_catalogs: &'a [PathBuf],
    /// The appearance rows of the settings page, over the family folder they save into: the
    /// user's own in `run`, a folder of the test's own in a test.
    pub appearance: Appearance,
    /// snapd's socket, which Discover reads the snaps and a job's progress from as the user.
    pub snap_socket: &'a Path,
    /// The first-run wizard, when qpac has no settings file of its own yet; `None` for someone
    /// who has one, and for a test or a picture that shows the screen after it.
    pub first_run: Option<FirstRun>,
}

/// Where the application reads the system's files and keeps the user's own, beside what the
/// [`Machine`] gives: the real places when it runs, a scratch folder in a test.
#[derive(Debug, Clone)]
pub struct Places {
    /// The root of the file system the snapshot tools and reflector's timer are looked for
    /// under: `/` on a real machine.
    pub root: PathBuf,
    /// The user's systemd unit folder, where the background check's timer is written; `None`
    /// without a home folder.
    pub units: Option<PathBuf>,
    /// qpac's own absolute path, which the background check's service runs and paru or yay call
    /// in place of sudo while they build; `None` when it cannot be told.
    pub exe: Option<PathBuf>,
    /// The user's runtime folder, where an AUR build's private pipes are made.
    pub runtime: Option<PathBuf>,
    /// The user's home folder, whose build cache paru and yay build in.
    pub home: Option<PathBuf>,
    /// The user's cache folder, where the AUR recipes to review are cloned; `None` without one.
    pub cache: Option<PathBuf>,
    /// The user's data folder, where the approved recipes are kept; `None` without one.
    pub data: Option<PathBuf>,
}

impl Places {
    /// The places of the machine qpac runs on.
    #[must_use]
    pub fn real() -> Self {
        Self {
            root: PathBuf::from("/"),
            units: crate::autostart::unit_dir(|name| std::env::var(name).ok()),
            exe: std::env::current_exe().ok(),
            runtime: Some(runtime_dir()),
            home: std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute()),
            cache: storage::cache_dir(Family::QUVYTA.id()),
            data: storage::data_dir(Family::QUVYTA.id()),
        }
    }

    /// Where the AUR recipe review reads and writes, when the user has both folders.
    fn review(&self) -> Option<review::Places> {
        review::Places::new(self.cache.as_deref(), self.data.as_deref())
    }

    /// What an AUR build run by the user `uid` needs, when everything is known.
    fn build(&self, uid: Option<u32>) -> Option<transaction::BuildPlaces> {
        Some(transaction::BuildPlaces {
            exe: self.exe.clone()?,
            runtime: self.runtime.clone()?,
            home: self.home.clone()?,
            uid: uid?,
        })
    }
}

/// `$XDG_RUNTIME_DIR` when it is an absolute path, which only the user may enter; otherwise the
/// system's temporary folder, where the build's folder is still the user's alone.
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
}

/// The application's state.
pub struct Qpackages {
    /// What the last read found; `None` until the first read answers.
    library: Option<Snapshot>,
    /// The read that fills the screen, repeated after every change.
    reload: Reload,
    /// The update check, run once the sources are known and whenever the user asks.
    checker: Checker,
    /// Whether the first check was started.
    checked_once: bool,
    /// Finds programs, for choosing the permission tool again when its setting changes.
    lookup: Arc<Lookup>,
    /// Whether the application runs as root, which makes the AUR unusable.
    root: bool,
    /// The user id the application runs as, when it could be read.
    uid: Option<u32>,
    /// The settings, changed and saved by the settings page.
    settings: Settings,
    /// The open tab.
    tab: Tab,
    /// Whether the settings page stands in place of the open tab.
    settings_open: bool,
    discover: store::Store,
    installed: Installed,
    updates: Updates,
    utc_offset: i16,
    /// The screen, as the runtime last reported it.
    size: Size,
    /// The transaction flow.
    transaction: Flow,
    /// Runs the programs qpac asks things of by itself, such as the news feed's download.
    runner: Arc<dyn Runner>,
    /// Where the system's files are.
    places: Places,
    /// Which snapshot tools this machine has, as last looked at.
    detected: backup::Detected,
    /// Whether the ladder's install step can be chosen here, as last looked at.
    pub(crate) ladder: crate::ladder::Here,
    /// Who must own the install step's script and the folders above it: root, except on a
    /// pretend machine a test builds, whose files its own user owns.
    system_owner: u32,
    /// What the settings page knows about reflector.
    reflector: Reflector,
    /// The appearance rows and the shared preferences behind them.
    appearance: Appearance,
    /// The first-run wizard while it has the screen; `None` once it is over, and from the start
    /// for someone who has `packages.conf` already.
    setup: Option<Setup<Msg>>,
    /// The family folder the wizard writes into, which the appearance rows save into after it.
    setup_folder: Option<PathBuf>,
    /// What the wizard's own steps hold until Finish.
    choices: wizard::Choices,
    /// What the Settings page would start for the sources the wizard was asked to bring, taken
    /// one after another once it is over.
    after_setup: VecDeque<settings_page::Msg>,
    /// Where the family's switch for the notice of a newer qpac is kept, and whether it is on;
    /// `None` where qpac asks about itself not at all.
    self_update: Option<self_update::SelfUpdate>,
}

impl std::fmt::Debug for Qpackages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Qpackages")
            .field("tab", &self.tab)
            .field("settings_open", &self.settings_open)
            .field("installed", &self.installed)
            .field("updates", &self.updates)
            .field("transaction", &self.transaction)
            .finish_non_exhaustive()
    }
}

/// Everything that can happen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A tab was chosen, by its place in [`TABS`].
    Tab(usize),
    /// The next tab to the right, if any.
    NextTab,
    /// The next tab to the left, if any.
    PrevTab,
    /// The settings page was asked for.
    OpenSettings,
    /// Puts the keyboard in the open page's search field.
    FocusSearch,
    /// Something happened in the Discover tab.
    Discover(store::Msg),
    /// Something happened in the Installed tab.
    Installed(installed::Msg),
    /// Something happened in the Updates tab.
    Updates(updates::Msg),
    /// Something happened on the settings page.
    Settings(settings_page::Msg),
    /// The screen has this size now.
    Resized(Size),
    /// The local database was read and the sources were looked for: when the application
    /// starts, and again after a transaction.
    Reloaded(Snapshot),
    /// The settings were written, or why not.
    Saved(Result<(), String>),
    /// Something happened in the transaction flow.
    Transaction(transaction::Msg),
    /// The orphans were asked to be shown: every package, where they are marked.
    ShowOrphans,
    /// Something on the first-run wizard's appearance step or its buttons, which the framework
    /// answers.
    Setup(SetupMsg),
    /// Something on one of qpac's own steps of the wizard.
    Wizard(WizardMsg),
    /// The wizard wrote the shared keys and made `packages.conf`, and is over.
    SetUp,
    /// crates.io has a newer qpac than the one running: qpac itself, not a package.
    NewerQpac(qframe::runtime::Update),
    /// The family's switch for that notice was written, or why not.
    SelfUpdateSaved(Result<(), String>),
}

/// What an application without its first read shows: nothing, not an empty machine.
static NO_PACKAGES: BTreeSet<String> = BTreeSet::new();
static NO_FOREIGN: Foreign = None;

impl Qpackages {
    /// The application on `machine`, following `settings` for which sources to use, which AUR
    /// helper to prefer and which program asks for administrator permission. Nothing is read
    /// yet: the packages and the sources arrive through [`App::init`], so the first frame is
    /// drawn before the database is opened.
    #[must_use]
    pub fn new(machine: Machine<'_>, settings: &Settings) -> Self {
        let tool = Tool::choose(settings::privilege_tool(settings), machine.lookup.as_ref());
        let reload = Reload::new(
            machine.dbpath,
            machine.applications,
            settings::aur_preference(settings),
            Arc::clone(&machine.lookup),
            Arc::clone(&machine.runner),
        );
        let checker = Checker::new(
            Arc::clone(&machine.runner),
            Arc::clone(&machine.lookup),
            machine.dbpath,
            machine.sync_dir,
            machine.check_dir,
        );
        let root = machine.uid == Some(ROOT);
        let store_machine = store::Machine {
            runner: Arc::clone(&machine.runner),
            swcatalog: machine.app_catalog.to_path_buf(),
            flatpak: machine.flatpak_catalogs.to_vec(),
        };
        let snapd = Snapd::new(false, machine.snap_socket);
        let (setup, setup_folder) = match machine.first_run {
            Some(first_run) => (Some(first_run.setup), Some(first_run.folder)),
            None => (None, None),
        };
        let mut app = Self {
            library: None,
            // The rankings are kept beside the update check's copy of pacman's database.
            discover: store::Store::new(store_machine, store_sources(settings, root))
                .with_cache(machine.check_dir.and_then(Path::parent).map(Path::to_path_buf)),
            checker,
            checked_once: false,
            lookup: Arc::clone(&machine.lookup),
            root,
            uid: machine.uid,
            settings: settings.clone(),
            tab: TABS[0],
            settings_open: false,
            installed: Installed::default(),
            updates: Updates::default(),
            utc_offset: machine.utc_offset,
            size: Size::default(),
            runner: Arc::clone(&machine.runner),
            places: Places::real(),
            detected: backup::Detected::default(),
            ladder: crate::ladder::Here::default(),
            system_owner: crate::ladder::ROOT,
            reflector: Reflector::default(),
            transaction: Flow::new(
                machine.runner,
                Session::new(machine.helper),
                tool,
                reload.clone(),
                machine.lock_dir,
            ),
            reload,
            appearance: machine.appearance,
            setup,
            setup_folder,
            choices: wizard::Choices::default(),
            after_setup: VecDeque::new(),
            self_update: None,
        };
        app.transaction.set_build_places(app.places.build(app.uid));
        app.transaction.set_review_places(app.places.review());
        // Both sides read the same socket: Discover for the snaps, the flow for a job's progress.
        app.discover.set_snapd(snapd.clone());
        app.transaction.set_snapd(snapd);
        app
    }

    /// The same application with `tab` open, for tests that start from another tab.
    #[cfg(test)]
    pub(crate) fn on_tab(mut self, tab: Tab) -> Self {
        self.tab = tab;
        self
    }

    /// The open tab.
    #[cfg(test)]
    pub(crate) fn tab(&self) -> Tab {
        self.tab
    }

    /// Whether the settings page is shown.
    #[cfg(test)]
    pub(crate) fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// The application with the system's files and the user's own at `places` instead of the
    /// real ones.
    #[must_use]
    pub fn with_places(mut self, places: Places) -> Self {
        self.transaction.set_build_places(places.build(self.uid));
        self.transaction.set_review_places(places.review());
        self.discover.set_snap_link(places.root.join(qpackages_core::snap::SNAP_LINK.trim_start_matches('/')));
        self.places = places;
        self
    }

    /// The same application on a pretend machine whose system files `owner` owns, so a test can
    /// put a trusted install script there.
    #[cfg(test)]
    pub(crate) fn system_owned_by(mut self, owner: u32) -> Self {
        self.system_owner = owner;
        self
    }

    /// Tells the flow which AUR helper builds: the one the last read found, while the AUR is in
    /// use.
    fn update_builder(&mut self) {
        let builder = self.library.as_ref().filter(|_| self.aur_in_use()).and_then(|found| {
            match (found.sources.aur_helper, found.sources.get(Source::Aur)) {
                (Some(helper), Availability::Ready { program }) => {
                    Some(transaction::Builder { helper, program: program.clone() })
                }
                _ => None,
            }
        });
        self.transaction.set_builder(builder);
    }

    /// What happens around a system update: the snapshot setting on what this machine has.
    fn backup_plan(&self) -> backup::Plan {
        backup::Plan::new(backend_settings::backup_tool(&self.settings, &self.detected), &self.detected)
    }

    /// Looks for the snapshot tools again and tells the flow what happens around an update.
    fn detect_backup(&mut self) {
        self.detected = backup::detect(&self.places.root);
        self.transaction.set_backup(self.backup_plan());
        // Looked at with the snapshot tools: a package that brings the install step's script is
        // noticed after the next read, like one that brings snapper.
        self.ladder = crate::ladder::Here::detect(&self.places.root, self.system_owner, self.uid);
    }

    /// Reads Arch's news of the last two weeks in the background.
    fn fetch_news(&self) -> Command<Msg> {
        let runner = Arc::clone(&self.runner);
        Command::perform(move || Msg::Updates(updates::Msg::News(read_news(runner.as_ref()))))
    }

    /// The installed packages as the pages need them; empty before the first read.
    fn library(&self) -> Library<'_> {
        library_of(self.library.as_ref())
    }

    /// Whether the AUR is turned on and usable from this account.
    fn aur_in_use(&self) -> bool {
        !self.root && settings::source_enabled(&self.settings, Source::Aur)
    }

    /// Starts an update check in the background: the repositories always, the AUR through its
    /// helper when it is in use and this machine has one.
    fn check(&self) -> Command<Msg> {
        let helper = self
            .library
            .as_ref()
            .and_then(|found| found.sources.aur_helper)
            .filter(|_| self.aur_in_use())
            .map(qpackages_core::sources::AurHelper::program);
        let checker = self.checker.clone();
        // snapd is asked only when Snap is on and it is really answering: without that the `snap`
        // program would retry for two minutes before saying anything.
        let snapd = self
            .discover
            .snap_state()
            .is_ready()
            .then(|| self.discover.snapd().clone())
            .filter(|_| settings::source_enabled(&self.settings, Source::Snap));
        let check = Command::perform(move || Msg::Updates(updates::Msg::Checked(checker.run(helper, snapd.as_ref()))));
        Command::batch([check, self.fetch_news()])
    }

    /// Does what the Updates tab asked.
    fn updates_message(&mut self, msg: updates::Msg) -> Command<Msg> {
        let checked = matches!(msg, updates::Msg::Checked(_));
        let request = self.updates.update(msg);
        if checked {
            self.transaction.set_pending_updates(self.updates.upgradable().len());
        }
        match request {
            Some(updates::Request::Check) => self.check(),
            Some(updates::Request::UpdateAll(list, snaps)) => {
                // Two steps, each confirmed for itself: pacman's transaction, then snapd's.
                let mut actions = Vec::new();
                if !list.is_empty() {
                    actions.push(Action::Upgrade(list));
                }
                if !snaps.is_empty() {
                    actions.push(Action::SnapRefresh(snaps));
                }
                self.update(Msg::Transaction(transaction::Msg::Queue(actions)))
            }
            None => Command::none(),
        }
    }

    /// Does what the last transaction left to do once the packages were read again: an update
    /// is followed by a new check, since what it installed is no longer waiting.
    fn after_transaction(&mut self) -> Command<Msg> {
        let Some(done) = self.transaction.take_done() else {
            return Command::none();
        };
        let check = if done.upgraded && self.updates.start() == Some(updates::Request::Check) {
            self.check()
        } else {
            Command::none()
        };
        let orphans: Vec<String> = match (&self.library, done.orphans) {
            (Some(found), true) => found.orphans.iter().flatten().cloned().collect(),
            _ => Vec::new(),
        };
        if orphans.is_empty() {
            return check;
        }
        let orphans = match backend_settings::orphans(&self.settings) {
            Orphans::Never => Command::none(),
            Orphans::Ask => {
                let clean = Msg::Transaction(transaction::Msg::Begin(Action::RemoveOrphans(orphans.clone())));
                let notice = Toast::info(t!("orphans.remain", n = orphans.len()))
                    .body(t!("orphans.remain-body"))
                    .action(t!("installed.clean-up"), clean)
                    .on_press(Msg::ShowOrphans)
                    .key("orphans");
                Command::toast(notice)
            }
            Orphans::Auto => {
                self.update(Msg::Transaction(transaction::Msg::BeginConfirmed(Action::RemoveOrphans(orphans))))
            }
        };
        Command::batch([check, orphans])
    }

    /// Does what Discover asked: an install or a removal goes to the transaction flow as the
    /// actions it takes, one after another, the sources to the settings page.
    fn discover_message(&mut self, msg: store::Msg) -> Command<Msg> {
        match msg {
            store::Msg::Request(store::Request::OpenSettings(_)) => self.update(Msg::OpenSettings),
            store::Msg::Request(request) => {
                let actions = self.discover.transactions(&request);
                if actions.is_empty() {
                    return Command::none();
                }
                self.update(Msg::Transaction(transaction::Msg::Queue(actions)))
            }
            msg => self.discover.update(msg).map(Msg::Discover),
        }
    }

    /// Does what the Installed tab asked.
    fn installed_message(&mut self, msg: installed::Msg) -> Command<Msg> {
        match self.installed.update(msg, library_of(self.library.as_ref())) {
            Some(installed::Request::Remove(names)) => {
                self.update(Msg::Transaction(transaction::Msg::Begin(Action::Remove(names))))
            }
            Some(installed::Request::CleanOrphans(names)) => {
                self.update(Msg::Transaction(transaction::Msg::Begin(Action::RemoveOrphans(names))))
            }
            None => Command::none(),
        }
    }

    /// Does what the settings page asked, saving the settings whenever one changed.
    fn settings_message(&mut self, msg: settings_page::Msg) -> Command<Msg> {
        match msg {
            settings_page::Msg::Back => {
                self.settings_open = false;
                Command::none()
            }
            settings_page::Msg::Source(source, on) => {
                let changed = settings::set_source_enabled(&mut self.settings, source, on);
                self.discover.set_enabled(store_sources(&self.settings, self.root));
                self.update_builder();
                self.saved_if(changed)
            }
            settings_page::Msg::AurHelper(index) => {
                let Some(&(preference, _)) = AUR_HELPERS.get(index) else {
                    return Command::none();
                };
                if !settings::set_aur_preference(&mut self.settings, preference) {
                    return Command::none();
                }
                self.prefer(preference);
                Command::batch([self.save(), self.reload.command()])
            }
            settings_page::Msg::PrivilegeTool(index) => {
                let Some(&(tool, _)) = PRIVILEGE_TOOLS.get(index) else {
                    return Command::none();
                };
                let changed = settings::set_privilege_tool(&mut self.settings, tool);
                self.transaction.set_tool(Tool::choose(tool, self.lookup.as_ref()));
                self.saved_if(changed)
            }
            settings_page::Msg::Install(source) => match sources::package(source) {
                Some(package) => {
                    let install = Action::Install(vec![package.to_owned()]);
                    self.update(Msg::Transaction(transaction::Msg::Begin(install)))
                }
                None => Command::none(),
            },
            settings_page::Msg::BuildFromAur(source) => match sources::aur_package(source) {
                Some(package) => {
                    let build = Action::AurInstall(vec![package.to_owned()]);
                    self.update(Msg::Transaction(transaction::Msg::Begin(build)))
                }
                None => Command::none(),
            },
            // The socket and the `/snap` link go one after another, each with its own
            // confirmation: without the link a snap with classic confinement cannot install, and
            // both are the same kind of one-off setting of the machine.
            settings_page::Msg::SnapSocket(on) => {
                let mut actions = vec![Action::SnapdSocket(on)];
                if on {
                    actions.push(Action::SnapLink);
                }
                self.update(Msg::Transaction(transaction::Msg::Queue(actions)))
            }
            settings_page::Msg::AddFlathub => {
                self.update(Msg::Transaction(transaction::Msg::Begin(Action::AddFlathub)))
            }
            settings_page::Msg::Backend(msg) => self.backend_setting(msg),
            // The appearance rows save themselves, each file read again right before it is
            // written, and keep the settings held here in step; there is nothing to save after.
            settings_page::Msg::Appearance(change) => self.appearance.update(change, &mut self.settings),
            settings_page::Msg::SelfUpdate(on) => self.switch_self_update(on),
        }
    }

    /// Uses `preference` for the AUR helper from the next read on, in the flow's reads as well.
    fn prefer(&mut self, preference: AurPreference) {
        self.reload.set_preference(preference);
        self.transaction.set_reload(self.reload.clone());
    }

    /// Writes the settings in the background.
    fn save(&self) -> Command<Msg> {
        self.settings.save_command(Msg::Saved)
    }

    /// Writes the settings when `changed`; nothing to write otherwise.
    fn saved_if(&self, changed: bool) -> Command<Msg> {
        if changed { self.save() } else { Command::none() }
    }

    /// The header: the name, the tabs with the number of waiting updates on the Updates tab, and
    /// on the right the root warning, the administrator badge and the settings button.
    fn header(&self, ui: &mut View<'_, Msg>) {
        let pending = self.updates.pending(self.aur_in_use());
        ui.row(|ui| {
            // The name, the tabs and the count fill what the controls on the right leave: those
            // are measured first, so a narrow screen scrolls the tabs rather than losing the
            // settings button.
            ui.row(|ui| {
                ui.add(Text::new("qpac").color("accent").bold().no_wrap());
                let labels = TABS.map(Tab::label);
                let count = u32::try_from(pending).unwrap_or(u32::MAX);
                // The count belongs to the Updates tab, so it keeps its place whatever tab is
                // last and a narrow strip cuts the tab's name before its count.
                let tabs = Tabs::new(labels).active(self.tab.index()).badge(Tab::Updates.index(), count);
                ui.add(tabs.on_select(Msg::Tab)).id("tabs");
            })
            .gap(2)
            .fill_width();
            if self.root {
                ui.add(Badge::new(t!("root.warning")).variant("warning"));
            }
            if self.transaction.has_helper() {
                let glyph = privilege_glyph(ui.env().icons().mode());
                let end = Button::new(format!("{glyph} {}", t!("helper.badge")))
                    .variant("warning")
                    .on_press(Msg::Transaction(transaction::Msg::EndHelper));
                ui.add_with(Tooltip::new(t!("helper.badge-tip")), |ui| {
                    ui.add(end).id("helper");
                });
            }
            let open = IconButton::new("settings").tooltip(t!("tabs.settings-tip")).on_press(Msg::OpenSettings);
            ui.add(open).id("open-settings");
        })
        .gap(2)
        .padding(Padding::symmetric(0, 2))
        .fill_width();
    }

    fn body(&self, ui: &mut View<'_, Msg>) {
        ui.column(|ui| {
            if self.transaction.shows_output() {
                let layout = output_layout(ui.size(), self.transaction.output_height());
                Splitter::rows(layout.packages)
                    .limits(MIN_PACKAGES, layout.widest)
                    .on_resize(move |rows| Msg::Transaction(transaction::Msg::OutputHeight(layout.pane_rows_for(rows))))
                    .first(|ui| self.page(ui))
                    .second(|ui| flow_view::output(&self.transaction, ui))
                    .show(ui)
                    .id("output-split");
            } else {
                self.page(ui);
            }
            flow_view::modal(&self.transaction, self.updates.news(), ui);
        })
        .fill();
    }

    /// The settings page when it is open, the open tab's page otherwise. Every tab's page keeps
    /// its widgets' state (scroll, the keyboard's place) while another is shown.
    fn page(&self, ui: &mut View<'_, Msg>) {
        // The recipes of a confirmed AUR build are read in place of the open tab: it is the one
        // decision the user is on, and the page under it has nothing to add to it.
        if let Some(screen) = self.transaction.reviewing() {
            ui.page("review", |ui| {
                ui.map(|msg| Msg::Transaction(transaction::Msg::Review(msg)), |ui| review::view(screen, ui)).fill();
            });
            return;
        }
        if self.settings_open {
            let cx = settings_page::Cx {
                settings: &self.settings,
                sources: self.library.as_ref().map(|found| &found.sources),
                flathub: self.library.as_ref().and_then(|found| found.flathub),
                snap: Some(self.discover.snap_state()),
                root: self.root,
                planning: self.transaction.is_planning(),
                tool: self.transaction.tool(),
                detected: &self.detected,
                ladder: &self.ladder,
                background: self.places.units.is_some() && self.places.exe.is_some(),
                reflector: &self.reflector,
                busy: !self.transaction.is_idle(),
                appearance: &self.appearance,
                self_update: self.self_update.as_ref().map(self_update::SelfUpdate::on),
            };
            ui.page("settings", |ui| {
                ui.map(Msg::Settings, |ui| settings_page::view(ui, cx)).fill();
            });
            return;
        }
        ui.page(self.tab.key(), |ui| match self.tab {
            Tab::Discover => {
                ui.map(Msg::Discover, |ui| self.discover.view(ui)).fill();
            }
            Tab::Installed => {
                let cx = installed::Cx {
                    library: self.library(),
                    problems: self.library.as_ref().map_or(0, |found| found.problems.len()),
                    loading: self.library.is_none(),
                    planning: self.transaction.is_planning(),
                };
                ui.map(Msg::Installed, |ui| self.installed.view(ui, cx)).fill();
            }
            Tab::Updates => {
                let cx = updates::Cx {
                    aur: self.aur_in_use(),
                    utc_offset: self.utc_offset,
                    can_check: self.library.is_some(),
                    busy: !self.transaction.is_idle(),
                    backup: self.backup_plan(),
                };
                ui.map(Msg::Updates, |ui| self.updates.view(ui, cx)).fill();
            }
        });
    }

    /// The key hints of what is on screen; the remove key is named only while there is something
    /// checked to remove.
    fn footer(&self, ui: &mut View<'_, Msg>) {
        let mut hints = KeyHints::new();
        if self.settings_open {
            hints = hints.action(Scope::App, "back");
        } else if self.tab == Tab::Discover {
            hints = hints.action(Scope::App, "search").hint("space", t!("hints.check"));
            if self.discover.has_checks() {
                hints = hints.action(Scope::App, "install-checked");
            }
            if self.discover.can_go_back() {
                hints = hints.action(Scope::App, "back");
            }
        } else if self.tab == Tab::Installed {
            hints = hints.action(Scope::App, "search").hint("space", t!("hints.check"));
            if self.installed.has_checks() {
                hints = hints.action(Scope::App, "remove");
            }
        }
        let hints = hints.action(Scope::App, "tab-next").action(Scope::App, "settings");
        ui.add(hints.action(Scope::Global, "focus-next").action_right(Scope::Global, "quit")).fill_width();
    }
}

/// The sources Discover offers: the ones the settings keep on, without the AUR for root, who
/// cannot build its packages.
fn store_sources(settings: &Settings, root: bool) -> Vec<Source> {
    sources::ALL
        .into_iter()
        .filter(|&source| settings::source_enabled(settings, source) && !(root && source == Source::Aur))
        .collect()
}

/// The installed packages of `found` as the pages need them; empty before the first read.
fn library_of(found: Option<&Snapshot>) -> Library<'_> {
    match found {
        Some(found) => {
            Library { packages: &found.packages, apps: &found.apps, foreign: &found.foreign, orphans: &found.orphans }
        }
        None => Library { packages: &[], apps: &NO_PACKAGES, foreign: &NO_FOREIGN, orphans: &NO_FOREIGN },
    }
}

/// Arch's news of the last two weeks, read with curl; what curl said when it could not.
fn read_news(runner: &dyn Runner) -> Result<Vec<NewsItem>, String> {
    let output = runner.output(CURL, &curl_args(NEWS_URL), &[]).map_err(|error| error.to_string())?;
    if !output.succeeded() {
        return Err(output.stderr.trim().to_owned());
    }
    let items = parse_news(&output.stdout).map_err(|problem| problem.to_string())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    Ok(recent(&items, i64::try_from(now).unwrap_or(i64::MAX)))
}

/// The mark before the word that says the helper is up: a lock where the font has one.
fn privilege_glyph(mode: GlyphMode) -> &'static str {
    match mode {
        GlyphMode::Nerd => "\u{f033e}",
        GlyphMode::Unicode => "◆",
        GlyphMode::Ascii => "*",
    }
}

impl App for Qpackages {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        // The sources the wizard was asked to bring go one after another: the next one starts
        // once the flow has come to rest after the one before it and the packages were read again.
        let rests = matches!(msg, Msg::Transaction(_) | Msg::Reloaded(_));
        let command = self.handle(msg);
        if rests && !self.after_setup.is_empty() {
            return Command::batch([command, self.next_after_setup()]);
        }
        command
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        // The first start asks before it shows the packages: the wizard has the screen to itself.
        if self.setting_up() {
            self.setup_wizard(ui);
            return;
        }
        AppShell::new().header(|ui| self.header(ui)).body(|ui| self.body(ui)).footer(|ui| self.footer(ui)).show(ui);
    }

    fn action(&self, name: &str) -> Option<Msg> {
        // While the wizard asks, qpac's own keys have nothing to act on: its pages are not there.
        if self.setting_up() {
            return None;
        }
        self.key(name)
    }

    fn resized(&self, size: Size) -> Option<Msg> {
        Some(Msg::Resized(size))
    }

    fn init(&mut self) -> Command<Msg> {
        // The packages are read even while the wizard asks: its sources step shows what this
        // machine has.
        let reads = Command::batch([self.reload.command(), self.discover.init().map(Msg::Discover)]);
        if self.setting_up() {
            // Nothing is asked about qpac itself while the wizard is open: a first start has
            // enough to say, and the next start asks.
            return Command::batch([reads, Command::focus(wizard::FIRST)]);
        }
        Command::batch([reads, self.ask_for_newer_qpac()])
    }
}

impl Qpackages {
    /// Applies `msg`.
    fn handle(&mut self, msg: Msg) -> Command<Msg> {
        match msg {
            Msg::Tab(index) => {
                if let Some(&tab) = TABS.get(index) {
                    self.tab = tab;
                    self.settings_open = false;
                }
            }
            Msg::NextTab => return self.update(Msg::Tab(self.tab.index() + 1)),
            Msg::PrevTab => {
                if let Some(index) = self.tab.index().checked_sub(1) {
                    return self.update(Msg::Tab(index));
                }
            }
            Msg::OpenSettings => {
                self.settings_open = true;
                return self.look_at_reflector();
            }
            Msg::ShowOrphans => {
                self.settings_open = false;
                self.tab = Tab::Installed;
                let all = Show::ALL.iter().position(|show| *show == Show::All).unwrap_or(0);
                return self.installed_message(installed::Msg::Show(all));
            }
            Msg::FocusSearch => {
                if !self.settings_open && self.tab == Tab::Installed {
                    return Command::focus("search");
                }
            }
            Msg::Discover(msg) => return self.discover_message(msg),
            Msg::Installed(msg) => return self.installed_message(msg),
            Msg::Updates(msg) => return self.updates_message(msg),
            Msg::Settings(msg) => return self.settings_message(msg),
            Msg::Resized(size) => self.size = size,
            Msg::Reloaded(snapshot) => {
                let flatpaks = self
                    .discover
                    .machine_read(&snapshot.sources, snapshot.packages.iter().map(|p| p.name.clone()))
                    .map(Msg::Discover);
                self.library = Some(snapshot);
                self.update_builder();
                self.installed.reloaded(library_of(self.library.as_ref()));
                self.detect_backup();
                // An installation may have brought reflector.
                let reflector = if self.settings_open { self.look_at_reflector() } else { Command::none() };
                if !self.checked_once {
                    self.checked_once = true;
                    if self.updates.start() == Some(updates::Request::Check) {
                        return Command::batch([flatpaks, reflector, self.check()]);
                    }
                }
                return Command::batch([flatpaks, reflector, self.after_transaction()]);
            }
            Msg::Saved(Ok(())) => {}
            Msg::Saved(Err(reason)) => {
                return Command::toast(Toast::warning(t!("settings-page.not-saved")).body(reason));
            }
            Msg::Transaction(msg) => {
                // pacman gets a terminal the size of the pane its output fills; the same rule
                // lays that pane out in `body`.
                let pty = output_layout(self.size, self.transaction.output_height()).pty;
                let command = self.transaction.update(msg, pty);
                // Switching reflector's timer changes no package, so no read follows it; its
                // state is looked at again whenever the flow comes to rest.
                if self.transaction.is_idle() {
                    self.reflector.timer_on = backend::timer_enabled(&self.places.root);
                }
                return command;
            }
            // The framework owns its step: it applies the change, writes the shared keys and
            // makes `packages.conf` when the wizard finishes, and answers with `Msg::SetUp`.
            Msg::Setup(msg) => {
                if let Some(mut setup) = self.setup.take() {
                    let done = setup.update(msg, &mut self.settings);
                    self.setup = Some(setup);
                    return done;
                }
            }
            Msg::Wizard(msg) => self.update_wizard(msg),
            Msg::SetUp => return self.finish_setup(),
            Msg::NewerQpac(update) => return Command::toast(self_update::notice(&update)),
            Msg::SelfUpdateSaved(Ok(())) => {}
            Msg::SelfUpdateSaved(Err(reason)) => return self.self_update_not_saved(reason),
        }
        Command::none()
    }

    /// What the key named `name` does on the screen after the wizard.
    fn key(&self, name: &str) -> Option<Msg> {
        if !self.settings_open
            && self.tab == Tab::Discover
            && let Some(msg) = self.discover.action(name)
        {
            return Some(Msg::Discover(msg));
        }
        match name {
            "search" => Some(Msg::FocusSearch),
            "remove" if !self.settings_open && self.tab == Tab::Installed => {
                self.installed.remove_request().map(|_| Msg::Installed(installed::Msg::RemoveChecked))
            }
            "settings" => Some(Msg::OpenSettings),
            "tab-next" => Some(Msg::NextTab),
            "tab-prev" => Some(Msg::PrevTab),
            "back" if self.transaction.reviewing().is_some() => {
                Some(Msg::Transaction(transaction::Msg::Review(review::Msg::Cancel)))
            }
            "back" if self.settings_open => Some(Msg::Settings(settings_page::Msg::Back)),
            "back" if self.tab == Tab::Installed && self.installed.detail_open() => {
                Some(Msg::Installed(installed::Msg::CloseDetail))
            }
            _ => name.strip_prefix("tab-").and_then(|n| n.parse::<usize>().ok()).and_then(|n| {
                // `tab-1` is the first tab; a number past the last tab does nothing.
                n.checked_sub(1).filter(|index| *index < TABS.len()).map(Msg::Tab)
            }),
        }
    }
}

#[cfg(test)]
mod tests;

//! The frame: the application's name, the tabs and the settings button on top, the open tab's
//! page (or the settings page) below, the key hints at the bottom; a transaction's confirmation
//! over it all and its output under the page.
//!
//! Each page is its own module with its own messages and state; the frame owns what they share
//! (the installed packages, the settings, the transaction flow) and does what they ask.

mod layout;
mod tab;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::storage::Settings;
use qframe::widgets::{Badge, Splitter, Tabs, Toast, Tooltip};
use qpackages_core::sources::{AurPreference, Source};

use crate::helper::pkexec::Tool;
use crate::helper::session::{Session, Start};
use crate::installed::table::Foreign;
use crate::installed::{self, Installed, Library};
use crate::reload::{Lookup, Reload, Snapshot};
use crate::runner::Runner;
use crate::settings::{self, AUR_HELPERS, PRIVILEGE_TOOLS};
use crate::settings_page::{self, Shared};
use crate::transaction::{self, Action, Flow};
use crate::updates::check::Checker;
use crate::updates::{self, Updates};
use crate::{sources, store, transaction::view as flow_view};

use layout::{MIN_PACKAGES, output_layout};
pub use tab::{TABS, Tab};

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
        Self {
            library: None,
            discover: store::Store::new(store_machine, store_sources(settings, root)),
            checker,
            checked_once: false,
            lookup: Arc::clone(&machine.lookup),
            root,
            settings: settings.clone(),
            tab: TABS[0],
            settings_open: false,
            installed: Installed::default(),
            updates: Updates::default(),
            utc_offset: machine.utc_offset,
            size: Size::default(),
            transaction: Flow::new(
                machine.runner,
                Session::new(machine.helper),
                tool,
                reload.clone(),
                machine.lock_dir,
            ),
            reload,
        }
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
        Command::perform(move || Msg::Updates(updates::Msg::Checked(checker.run(helper))))
    }

    /// Does what Discover asked: an install or a removal goes to the transaction flow when the
    /// flow can run it, the sources to the settings page.
    fn discover_message(&mut self, msg: store::Msg) -> Command<Msg> {
        match msg {
            store::Msg::Request(store::Request::OpenSettings(_)) => self.update(Msg::OpenSettings),
            store::Msg::Request(request) => match store::transaction(&request) {
                Some(action) => self.update(Msg::Transaction(transaction::Msg::Begin(action))),
                None => Command::none(),
            },
            msg => self.discover.update(msg).map(Msg::Discover),
        }
    }

    /// Does what the Installed tab asked.
    fn installed_message(&mut self, msg: installed::Msg) -> Command<Msg> {
        match self.installed.update(msg, library_of(self.library.as_ref())) {
            Some(installed::Request::Remove(names)) => {
                self.update(Msg::Transaction(transaction::Msg::Begin(Action::Remove(names))))
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
            settings_page::Msg::Shared(change) => {
                let (changed, apply) = match change {
                    Shared::Language(code) => {
                        (self.settings.set(Settings::LANGUAGE, code.clone()), Command::set_locale(code))
                    }
                    Shared::Theme(id) => (self.settings.set(Settings::THEME, id.clone()), Command::set_theme(id)),
                    Shared::Icons(mode) => {
                        (self.settings.set(Settings::ICONS, mode.name().to_owned()), Command::set_icon_mode(mode))
                    }
                    Shared::ReducedMotion(on) => {
                        (self.settings.set(Settings::REDUCED_MOTION, on), Command::set_reduced_motion(on))
                    }
                    Shared::Pillar(style) => {
                        (self.settings.set(Settings::PILLAR, style.name().to_owned()), Command::set_pillar(style))
                    }
                };
                Command::batch([apply, self.saved_if(changed)])
            }
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

    /// The header: the name, the tabs with the number of waiting updates, and on the right the
    /// root warning, the administrator badge and the settings button.
    fn header(&self, ui: &mut View<'_, Msg>) {
        let pending = self.updates.pending(self.aur_in_use());
        ui.row(|ui| {
            // The name, the tabs and the count fill what the controls on the right leave: those
            // are measured first, so a narrow screen scrolls the tabs rather than losing the
            // settings button.
            ui.row(|ui| {
                ui.add(Text::new("qpac").color("accent").bold().no_wrap());
                let labels = TABS.map(Tab::label);
                ui.add(Tabs::new(labels).active(self.tab.index()).on_select(Msg::Tab)).id("tabs");
                // The Updates tab is the last one, so the count stands right after its label.
                if pending > 0 {
                    let count = u32::try_from(pending).unwrap_or(u32::MAX);
                    ui.add(Badge::new("").variant("accent").count(count)).id("pending");
                }
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
            let gear = ui.env().icons().glyph("settings").into_owned();
            let open = Button::new(gear).selected(self.settings_open).on_press(Msg::OpenSettings);
            ui.add_with(Tooltip::new(t!("tabs.settings-tip")), |ui| {
                ui.add(open).id("open-settings");
            });
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
            flow_view::modal(&self.transaction, ui);
        })
        .fill();
    }

    /// The settings page when it is open, the open tab's page otherwise. Every tab's page keeps
    /// its widgets' state (scroll, the keyboard's place) while another is shown.
    fn page(&self, ui: &mut View<'_, Msg>) {
        if self.settings_open {
            let cx = settings_page::Cx {
                settings: &self.settings,
                sources: self.library.as_ref().map(|found| &found.sources),
                root: self.root,
                planning: self.transaction.is_planning(),
                tool: self.transaction.tool(),
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
        Some(found) => Library { packages: &found.packages, apps: &found.apps, foreign: &found.foreign },
        None => Library { packages: &[], apps: &NO_PACKAGES, foreign: &NO_FOREIGN },
    }
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
            Msg::OpenSettings => self.settings_open = true,
            Msg::FocusSearch => {
                if !self.settings_open && self.tab == Tab::Installed {
                    return Command::focus("search");
                }
            }
            Msg::Discover(msg) => return self.discover_message(msg),
            Msg::Installed(msg) => return self.installed_message(msg),
            Msg::Updates(msg) => {
                if self.updates.update(msg) == Some(updates::Request::Check) {
                    return self.check();
                }
            }
            Msg::Settings(msg) => return self.settings_message(msg),
            Msg::Resized(size) => self.size = size,
            Msg::Reloaded(snapshot) => {
                self.discover.machine_read(&snapshot.sources, snapshot.packages.iter().map(|p| p.name.clone()));
                self.library = Some(snapshot);
                self.installed.reloaded(library_of(self.library.as_ref()));
                if !self.checked_once {
                    self.checked_once = true;
                    if self.updates.start() == Some(updates::Request::Check) {
                        return self.check();
                    }
                }
            }
            Msg::Saved(Ok(())) => {}
            Msg::Saved(Err(reason)) => {
                return Command::toast(Toast::warning(t!("settings-page.not-saved")).body(reason));
            }
            Msg::Transaction(msg) => {
                // pacman gets a terminal the size of the pane its output fills; the same rule
                // lays that pane out in `body`.
                let pty = output_layout(self.size, self.transaction.output_height()).pty;
                return self.transaction.update(msg, pty);
            }
        }
        Command::none()
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        AppShell::new().header(|ui| self.header(ui)).body(|ui| self.body(ui)).footer(|ui| self.footer(ui)).show(ui);
    }

    fn action(&self, name: &str) -> Option<Msg> {
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

    fn resized(&self, size: Size) -> Option<Msg> {
        Some(Msg::Resized(size))
    }

    fn init(&mut self) -> Command<Msg> {
        Command::batch([self.reload.command(), self.discover.init().map(Msg::Discover)])
    }
}

#[cfg(test)]
mod tests;

//! The screen: the sources on the left, the installed packages with a search in the middle and
//! the selected package's details on the right; a transaction's confirmation over it and its
//! output below it.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use qframe::prelude::*;
use qframe::storage::Settings;
use qframe::widgets::{Badge, EmptyState, SortDirection, Splitter, Table, TableRow, TextInput};
use qpackages_core::pacman::{Package, Problem};
use qpackages_core::sources::{Availability, Source, Sources};

use crate::reload::{Lookup, Reload, Snapshot};
use crate::runner::Runner;
use crate::transaction::{self, Action, Flow};
use crate::{detail, packages, settings, sources};

mod layout;

use layout::{COLLAPSE, MIN_DETAIL, MIN_PACKAGES, MIN_TABLE, SIDEBAR, output_layout};

/// The user id of root, for whom the AUR is off limits.
const ROOT: u32 = 0;

/// The machine the application runs on, as its constructor needs it: the real one in `run`, a
/// pretend one in tests.
pub struct Machine<'a> {
    /// Where pacman keeps the records of installed packages.
    pub dbpath: &'a Path,
    /// The directory pacman's lock file lives in.
    pub lock_dir: &'a Path,
    /// Finds a program on this machine, as `on_path` does.
    pub lookup: Arc<Lookup>,
    /// Runs programs.
    pub runner: Arc<dyn Runner>,
    /// The user id the application runs as, or `None` when it could not be read.
    pub uid: Option<u32>,
}

/// The application's state.
#[derive(Debug)]
pub struct Qpackages {
    /// Every installed package, in name order.
    packages: Vec<Package>,
    /// Records the database read could not use.
    problems: Vec<Problem>,
    /// Which sources this machine has; `None` until the first read answers.
    sources: Option<Sources>,
    /// The read that fills the screen, repeated after every change.
    reload: Reload,
    /// The sources the user keeps in the sidebar, in sidebar order.
    shown_sources: Vec<Source>,
    /// The source chosen in the sidebar.
    source: Source,
    /// Whether the application runs as root, which makes the AUR unusable.
    root: bool,
    search: String,
    sort: (usize, SortDirection),
    /// Indices into `packages` of the rows shown, in table order.
    shown: Vec<usize>,
    /// The rows of `shown`, rebuilt only when the search or the sort changes.
    rows: Arc<[TableRow]>,
    /// The name of the selected package: a selection outlives a search that hides its row.
    selected: Option<String>,
    /// Names of the checked packages: a check survives a search that hides its row.
    checked: BTreeSet<String>,
    /// `checked` as the table wants it, one flag per shown row.
    marks: Vec<bool>,
    /// Width of the table pane, as the user dragged it.
    split: u16,
    /// The screen, as the runtime last reported it.
    size: Size,
    /// The transaction flow.
    transaction: Flow,
}

/// Everything that can happen.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The search text changed.
    Search(String),
    /// Puts the keyboard in the search field.
    FocusSearch,
    /// A source was chosen in the sidebar, by its index there.
    Source(usize),
    /// A row was selected.
    Select(usize),
    /// A row's check mark was toggled.
    Toggle(usize),
    /// The table was asked to sort by a column.
    Sort(usize, SortDirection),
    /// The boundary between the table and the details was dragged.
    Split(u16),
    /// The screen has this size now.
    Resized(Size),
    /// The user asked to remove the checked packages.
    RemoveChecked,
    /// The local database was read and the sources were looked for: when the application
    /// starts, and again after a transaction.
    Reloaded(Snapshot),
    /// Something happened in the transaction flow.
    Transaction(transaction::Msg),
}

impl Qpackages {
    /// The application on `machine`, following `settings` for which sources to show and which
    /// AUR helper to prefer. Nothing is read yet: the packages and the sources arrive through
    /// [`App::init`], so the first frame is drawn before the database is opened.
    #[must_use]
    pub fn new(machine: Machine<'_>, settings: &Settings) -> Self {
        let reload = Reload::new(machine.dbpath, settings::aur_preference(settings), machine.lookup);
        let shown_sources: Vec<Source> =
            sources::ALL.into_iter().filter(|source| settings::source_enabled(settings, *source)).collect();
        let mut app = Self {
            packages: Vec::new(),
            problems: Vec::new(),
            sources: None,
            source: shown_sources.first().copied().unwrap_or(Source::Pacman),
            shown_sources,
            root: machine.uid == Some(ROOT),
            search: String::new(),
            sort: (packages::NAME, SortDirection::Ascending),
            shown: Vec::new(),
            rows: Arc::from([]),
            selected: None,
            checked: BTreeSet::new(),
            marks: Vec::new(),
            split: 48,
            size: Size::default(),
            transaction: Flow::new(machine.runner, reload.clone(), machine.lock_dir),
            reload,
        };
        app.rebuild();
        app
    }

    /// Rebuilds the shown rows after the search or the sort changed.
    fn rebuild(&mut self) {
        let (column, direction) = self.sort;
        self.shown = packages::shown(&self.packages, &self.search, column, direction);
        self.rows = packages::rows(&self.packages, &self.shown);
        self.marks = self.shown.iter().map(|&i| self.checked.contains(&self.packages[i].name)).collect();
    }

    /// The row of the selected package, while the search shows it.
    fn selected_row(&self) -> Option<usize> {
        let name = self.selected.as_deref()?;
        self.shown.iter().position(|&i| self.packages[i].name == name)
    }

    /// The selected package, while the search shows it.
    fn selected_package(&self) -> Option<&Package> {
        self.selected_row().map(|row| &self.packages[self.shown[row]])
    }

    /// Whether `source` can be used from this account: the AUR cannot when running as root,
    /// because building packages as root is refused.
    fn usable(&self, source: Source) -> bool {
        !(self.root && source == Source::Aur)
    }

    /// Whether this machine has `source`, once the first read has said.
    fn availability(&self, source: Source) -> Option<&Availability> {
        self.sources.as_ref().map(|sources| sources.get(source))
    }

    /// Whether the first read is still on its way.
    fn is_loading(&self) -> bool {
        self.sources.is_none()
    }

    /// Starts removing the checked packages, when there are any.
    fn remove_checked(&self) -> Option<Msg> {
        if self.checked.is_empty() {
            return None;
        }
        let names = self.checked.iter().cloned().collect();
        Some(Msg::Transaction(transaction::Msg::Begin(Action::Remove(names))))
    }

    fn header(&self, ui: &mut View<'_, Msg>) {
        ui.row(|ui| {
            ui.add(Text::new("qpac").color("accent").bold().no_wrap());
            if self.root {
                ui.add(Badge::new(t!("root.warning")).variant("warning"));
            }
            ui.add(TextInput::new(self.search.clone()).placeholder(t!("search.placeholder")).on_change(Msg::Search))
                .fill_width()
                .id("search");
        })
        .gap(2)
        .padding(Padding::symmetric(0, 2))
        .fill_width();
    }

    fn sidebar(&self, ui: &mut View<'_, Msg>) {
        let items = self.shown_sources.iter().map(|&source| {
            let item = ListItem::new(t!(&format!("source.{}", sources::name(source))));
            // The word beside a faint row says why it is faint; colour alone never does.
            if !self.usable(source) {
                return item.faint(true).detail(t!("source.not-as-root"));
            }
            match self.availability(source) {
                // Nothing is said about a source before the read has looked for it.
                None | Some(Availability::Ready { .. }) => item,
                Some(Availability::Missing) => item.faint(true).detail(t!("source.missing")),
            }
        });
        let selected = self.shown_sources.iter().position(|&source| source == self.source);
        ui.add(List::new(items).selected(selected).on_select(Msg::Source)).fill().id("sources");
    }

    fn body(&self, ui: &mut View<'_, Msg>) {
        ui.column(|ui| {
            if self.transaction.shows_output() {
                let layout = output_layout(ui.size(), self.transaction.output_height());
                Splitter::rows(layout.packages)
                    .limits(MIN_PACKAGES, layout.widest)
                    .on_resize(move |rows| Msg::Transaction(transaction::Msg::OutputHeight(layout.pane_rows_for(rows))))
                    .first(|ui| self.source_body(ui))
                    .second(|ui| transaction::view::output(&self.transaction, ui))
                    .show(ui)
                    .id("output-split");
            } else {
                self.source_body(ui);
            }
            transaction::view::modal(&self.transaction, ui);
        })
        .fill();
    }

    fn source_body(&self, ui: &mut View<'_, Msg>) {
        let name = t!(&format!("source.{}", sources::name(self.source)));
        match (self.source, self.availability(self.source)) {
            (Source::Aur, _) if self.root => {
                let empty = EmptyState::new(t!("source.root-title", name = name))
                    .icon("warning")
                    .message(t!("source.root-message"));
                ui.add(empty).fill();
            }
            (source, Some(Availability::Missing)) => self.missing(source, &name, ui),
            // Before the read answers, the table stands with its loading line; it will fill.
            (Source::Pacman, _) => self.pacman(ui),
            (_, None) => {
                ui.add(EmptyState::new(t!("source.detecting", name = name)).icon("info")).fill();
            }
            (_, Some(Availability::Ready { .. })) => {
                let empty = EmptyState::new(t!("source.waiting-title", name = name))
                    .icon("info")
                    .message(t!("source.waiting-message"));
                ui.add(empty).fill();
            }
        }
    }

    /// A source this machine does not have: an offer to install it where its package is in the
    /// repositories, and the reason where it is not.
    fn missing(&self, source: Source, name: &str, ui: &mut View<'_, Msg>) {
        let empty = EmptyState::new(t!("source.missing-title", name = name)).icon("inbox");
        let empty = match sources::package(source) {
            Some(package) => {
                let install = Action::Install(vec![package.to_owned()]);
                let button = Button::new(t!("source.install", package = package))
                    .variant("primary")
                    .loading(self.transaction.is_planning())
                    .on_press(Msg::Transaction(transaction::Msg::Begin(install)));
                empty.message(t!("source.missing-message", package = package)).action(button)
            }
            None => empty.message(t!("source.missing-later")),
        };
        ui.add(empty).fill();
    }

    fn pacman(&self, ui: &mut View<'_, Msg>) {
        let width = ui.size().width;
        let body = if width < COLLAPSE { width } else { width.saturating_sub(SIDEBAR) };
        let widest = body.saturating_sub(MIN_DETAIL).max(MIN_TABLE);
        Splitter::columns(self.split.clamp(MIN_TABLE, widest))
            .limits(MIN_TABLE, widest)
            .on_resize(Msg::Split)
            .first(|ui| self.table(ui))
            .second(|ui| detail::view(self.selected_package(), ui))
            .show(ui);
    }

    fn table(&self, ui: &mut View<'_, Msg>) {
        ui.column(|ui| {
            if !self.problems.is_empty() {
                ui.add(Text::new(t!("packages.unreadable", n = self.problems.len())).role("faint").no_wrap())
                    .fill_width();
            }
            if !self.checked.is_empty() {
                ui.row(|ui| {
                    ui.add(Text::new(t!("transaction.checked", n = self.checked.len())).role("faint").no_wrap())
                        .fill_width();
                    let remove = Button::new(t!("transaction.remove"))
                        .variant("danger")
                        .loading(self.transaction.is_planning())
                        .on_press(Msg::RemoveChecked);
                    ui.add(remove).id("remove");
                })
                .gap(2)
                .fill_width();
            }
            let empty = if self.is_loading() {
                t!("packages.loading")
            } else if self.search.is_empty() {
                t!("packages.none-installed")
            } else {
                t!("packages.none-match")
            };
            let (column, direction) = self.sort;
            let table = Table::new(packages::columns(), Arc::clone(&self.rows))
                .selected(self.selected_row())
                .checked(self.marks.clone())
                .sort(column, direction)
                .empty_text(empty)
                .on_select(Msg::Select)
                .on_toggle(Msg::Toggle)
                .on_sort(Msg::Sort);
            ui.add(table).fill().id("packages");
        })
        .fill();
    }
}

impl App for Qpackages {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        match msg {
            Msg::Search(text) => {
                self.search = text;
                self.rebuild();
            }
            Msg::FocusSearch => return Command::focus("search"),
            Msg::Source(index) => {
                if let Some(&source) = self.shown_sources.get(index) {
                    self.source = source;
                }
            }
            Msg::Select(row) => {
                if let Some(&i) = self.shown.get(row) {
                    self.selected = Some(self.packages[i].name.clone());
                }
            }
            Msg::Toggle(row) => {
                if let Some(&i) = self.shown.get(row) {
                    let name = &self.packages[i].name;
                    if !self.checked.remove(name) {
                        self.checked.insert(name.clone());
                    }
                    self.marks[row] = self.checked.contains(name);
                }
            }
            Msg::Sort(column, direction) => {
                self.sort = (column, direction);
                self.rebuild();
            }
            Msg::Split(width) => self.split = width,
            Msg::Resized(size) => self.size = size,
            Msg::RemoveChecked => {
                if let Some(msg) = self.remove_checked() {
                    return self.update(msg);
                }
            }
            Msg::Reloaded(snapshot) => {
                self.packages = snapshot.packages;
                self.problems = snapshot.problems;
                self.sources = Some(snapshot.sources);
                // A package that is gone cannot stay checked or selected.
                let installed: BTreeSet<&str> = self.packages.iter().map(|package| package.name.as_str()).collect();
                self.checked.retain(|name| installed.contains(name.as_str()));
                if self.selected.as_deref().is_some_and(|name| !installed.contains(name)) {
                    self.selected = None;
                }
                self.rebuild();
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
        AppShell::new()
            .sidebar_width(SIDEBAR)
            .collapse_below(COLLAPSE)
            .header(|ui| self.header(ui))
            .sidebar(|ui| self.sidebar(ui))
            .body(|ui| self.body(ui))
            .footer(|ui| self.footer(ui))
            .show(ui);
    }

    fn action(&self, name: &str) -> Option<Msg> {
        match name {
            "search" => Some(Msg::FocusSearch),
            "remove" => self.remove_checked(),
            _ => None,
        }
    }

    fn resized(&self, size: Size) -> Option<Msg> {
        Some(Msg::Resized(size))
    }

    fn init(&mut self) -> Command<Msg> {
        self.reload.command()
    }
}

impl Qpackages {
    /// The key hints; the remove key is named only while there is something checked to remove.
    fn footer(&self, ui: &mut View<'_, Msg>) {
        let mut hints = KeyHints::new().action(Scope::App, "search").hint("space", t!("hints.check"));
        if !self.checked.is_empty() {
            hints = hints.action(Scope::App, "remove");
        }
        ui.add(hints.action(Scope::Global, "focus-next").action_right(Scope::Global, "quit")).fill_width();
    }
}

#[cfg(test)]
mod tests;

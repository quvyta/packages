//! The Installed tab: what is on this machine, as applications or as every package, with a
//! search, the selected package's details and the check marks a removal starts from.
//!
//! The tab owns only what the user did here: the search, the view, the sort, the selection and
//! the checks. The packages themselves belong to the application, which hands them in as a
//! [`Library`] whenever something here has to be worked out again.

pub mod apps;
mod query;
pub mod table;
mod view;

use std::collections::BTreeSet;

use qframe::widgets::SortDirection;
use qpackages_core::pacman::Package;

use query::Query;
use table::{Col, Foreign, Rows, Scope};
pub use view::{Cx, TEXT_INDENT};

/// What the tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// The packages that put a launcher in the application menu. The default: it is what most
    /// people mean by "what is installed".
    Apps,
    /// Every package, dependencies and libraries included.
    All,
}

impl Show {
    /// The choices in the order the segmented control offers them.
    pub const ALL: [Self; 2] = [Self::Apps, Self::All];
}

/// The installed packages as the tab needs them.
#[derive(Debug, Clone, Copy)]
pub struct Library<'a> {
    /// Every installed package, in name order.
    pub packages: &'a [Package],
    /// The names of the packages that are applications.
    pub apps: &'a BTreeSet<String>,
    /// Which packages no repository offers.
    pub foreign: &'a Foreign,
}

/// The tab's state.
#[derive(Debug)]
pub struct Installed {
    search: String,
    query: Query,
    show: Show,
    sort: (Col, SortDirection),
    /// Indices into the packages of the rows shown, in table order.
    shown: Vec<usize>,
    /// The table rows of `shown`, built when a frame first needs them.
    rows: Rows,
    /// The name of the selected package: a selection outlives a search that hides its row.
    selected: Option<String>,
    /// Names of the checked packages: a check survives a search or a view that hides its row.
    checked: BTreeSet<String>,
    /// `checked` as the table wants it, one flag per shown row.
    marks: Vec<bool>,
    /// Width of the table pane, once the user dragged it; until then it follows the screen.
    split: Option<u16>,
    /// Whether the details were opened in place of the table, which a narrow screen does.
    detail_open: bool,
}

/// Everything that can happen in the tab.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The search text changed.
    Search(String),
    /// The applications or every package, by position in `Show::ALL`.
    Show(usize),
    /// A row was selected.
    Select(usize),
    /// A row was opened, with Enter or a click: on a narrow screen its details take the tab.
    Open(usize),
    /// The details opened in place of the table were closed.
    CloseDetail,
    /// A row's check mark was toggled.
    Toggle(usize),
    /// The table was asked to sort by a column.
    Sort(Col, SortDirection),
    /// The boundary between the table and the details was dragged.
    Split(u16),
    /// The user asked to remove the checked packages.
    RemoveChecked,
}

/// What the tab asks of the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Remove these packages, through the transaction flow.
    Remove(Vec<String>),
}

impl Default for Installed {
    fn default() -> Self {
        Self {
            search: String::new(),
            query: Query::default(),
            show: Show::Apps,
            sort: (Col::Name, SortDirection::Ascending),
            shown: Vec::new(),
            rows: Rows::default(),
            selected: None,
            checked: BTreeSet::new(),
            marks: Vec::new(),
            split: None,
            detail_open: false,
        }
    }
}

impl Installed {
    /// Applies `msg` over `library` and says what the application should do, if anything.
    pub fn update(&mut self, msg: Msg, library: Library<'_>) -> Option<Request> {
        match msg {
            Msg::Search(text) => {
                self.query = Query::parse(&text);
                self.search = text;
                self.detail_open = false;
                self.rebuild(library);
            }
            Msg::Show(index) => {
                if let Some(&show) = Show::ALL.get(index) {
                    self.show = show;
                    self.detail_open = false;
                    self.rebuild(library);
                }
            }
            Msg::Select(row) => {
                if let Some(&i) = self.shown.get(row) {
                    self.selected = Some(library.packages[i].name.clone());
                }
            }
            Msg::Open(row) => {
                if let Some(&i) = self.shown.get(row) {
                    self.selected = Some(library.packages[i].name.clone());
                    self.detail_open = true;
                }
            }
            Msg::CloseDetail => self.detail_open = false,
            Msg::Toggle(row) => {
                if let Some(&i) = self.shown.get(row) {
                    let name = &library.packages[i].name;
                    if !self.checked.remove(name) {
                        self.checked.insert(name.clone());
                    }
                    self.marks[row] = self.checked.contains(name);
                }
            }
            Msg::Sort(col, direction) => {
                self.sort = (col, direction);
                self.rebuild(library);
            }
            Msg::Split(width) => self.split = Some(width),
            Msg::RemoveChecked => return self.remove_request(),
        }
        None
    }

    /// The packages changed: a check or a selection of a package that is gone goes with it.
    pub fn reloaded(&mut self, library: Library<'_>) {
        let installed: BTreeSet<&str> = library.packages.iter().map(|package| package.name.as_str()).collect();
        self.checked.retain(|name| installed.contains(name.as_str()));
        if self.selected.as_deref().is_some_and(|name| !installed.contains(name)) {
            self.selected = None;
            self.detail_open = false;
        }
        self.rebuild(library);
    }

    /// The request to remove the checked packages, when there are any.
    #[must_use]
    pub fn remove_request(&self) -> Option<Request> {
        (!self.checked.is_empty()).then(|| Request::Remove(self.checked.iter().cloned().collect()))
    }

    /// Whether the details stand in place of the table, so Escape has something to close.
    #[must_use]
    pub fn detail_open(&self) -> bool {
        self.detail_open
    }

    /// Whether any package is checked.
    #[must_use]
    pub fn has_checks(&self) -> bool {
        !self.checked.is_empty()
    }

    /// Works out the shown rows again after the search, the view, the sort or the packages
    /// changed.
    fn rebuild(&mut self, library: Library<'_>) {
        let (col, direction) = self.sort;
        let apps = (self.show == Show::Apps).then_some(library.apps);
        let scope = Scope { apps, foreign: library.foreign };
        self.shown = table::shown(library.packages, scope, &self.query, col, direction);
        self.rows.clear();
        self.marks = self.shown.iter().map(|&i| self.checked.contains(&library.packages[i].name)).collect();
    }

    /// The row of the selected package, while the search and the view show it.
    fn selected_row(&self, library: Library<'_>) -> Option<usize> {
        let name = self.selected.as_deref()?;
        self.shown.iter().position(|&i| library.packages[i].name == name)
    }
}

#[cfg(test)]
mod tests;

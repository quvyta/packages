//! Discover: the store's own page. What is popular, what a search finds in every source at
//! once, and one application's page with the button that installs or removes it.
//!
//! The page is a component with its own state, messages, `update` and `view`, mounted by the
//! application the framework's way: its view through [`View::map`] and the commands of its
//! `update` through [`Command::map`]. It never installs or removes anything itself. What the
//! person asks for leaves it as [`Msg::Request`], which the application takes before passing the
//! rest on:
//!
//! ```text
//! Msg::Store(store::Msg::Request(request)) => /* route: transaction flow, settings page */
//! Msg::Store(msg) => return self.store.update(msg).map(Msg::Store),
//! // in view:
//! ui.map(Msg::Store, |ui| self.store.view(ui)).fill();
//! ```
//!
//! Nothing is read before [`Store::init`], and everything read after runs in the background: the
//! first frame shows the built-in starter list, and the network's ranking replaces it in place.

mod card;
mod data;
mod model;
mod page;
mod view;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use qframe::prelude::*;
use qframe::runtime::{Task, TaskId};
use qpackages_core::catalog::aur::AurPackage;
use qpackages_core::catalog::featured::{self, FeaturedApp};
use qpackages_core::catalog::merge::{Offer, RepoPackage, id_key, merge};
use qpackages_core::catalog::popularity::{FlathubInstalls, PackageShare};
use qpackages_core::catalog::repo::RepoInfo;
use qpackages_core::catalog::search::{Generation, Generations, SearchIndex};
use qpackages_core::sources::{Source, Sources};

pub use data::{Failure, Loaded, Machine, SWCATALOG, flatpak_catalogs};
use model::{Card, Kind, Popularity, Sort};

/// The starter list, compiled in: it is what the home page shows before anything is read.
const FEATURED: &str = include_str!("../../assets/catalog/featured.toml");

/// The package that brings the repositories' AppStream catalog.
pub const CATALOG_PACKAGE: &str = "archlinux-appstream-data";

/// How long typing must pause before a search starts: a search per key would ask every source
/// for words nobody meant.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// How long a search runs before "Searching" and its spinner show; a quicker answer never
/// blinks one.
const SPINNER_AFTER: Duration = Duration::from_millis(300);

/// What an application from a catalog is lifted by in a search's ranking: more than any
/// popularity reaches (pkgstats counts in percent, the AUR's popularity stays far below it).
const CATALOG_FIRST: f64 = 1_000.0;

/// The most results laid out as cards. A two-letter term can match thousands of packages; past
/// this the person is better served typing more.
pub const MAX_RESULTS: usize = 300;

/// What the store asks the application to do. None of it happens inside the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Install these, each from its source, after the usual confirmation.
    Install(Vec<Offer>),
    /// Remove these, after the usual confirmation.
    Remove(Vec<Offer>),
    /// Open the settings, at the sources when one is named.
    OpenSettings(Option<Source>),
}

/// A home row, and the page that lists it whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// Popular applications from the repositories and Flathub.
    Popular,
    /// Popular packages only the AUR has.
    Aur,
}

/// A grid of cards on screen; each keeps its own selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grid {
    /// A home row.
    Row(Section),
    /// A section's whole list.
    All(Section),
    /// The search results.
    Results,
}

/// The pages of the store.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    /// Home, or the search results while there is a search.
    Home,
    /// One section's whole list.
    Section(Section),
    /// One application, by its card's key.
    App(String),
}

impl Page {
    /// The key the page transition and the page's own focus and scroll are kept under.
    fn key(&self) -> String {
        match self {
            Self::Home => String::from("home"),
            Self::Section(Section::Popular) => String::from("all-popular"),
            Self::Section(Section::Aur) => String::from("all-aur"),
            Self::App(key) => format!("app:{key}"),
        }
    }
}

/// What a source found for a search.
#[derive(Debug, Clone)]
pub enum Found {
    /// The repositories' packages.
    Repo(Arc<[RepoPackage]>),
    /// The AUR's packages.
    Aur(Arc<[AurPackage]>),
}

/// What is known about one offer of an open application, beyond its card.
#[derive(Debug, Clone)]
pub enum Details {
    /// `pacman -Si`.
    Repo(Box<RepoInfo>),
    /// The AUR's full record.
    Aur(Box<AurPackage>),
}

/// Everything that can happen on the page.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The search text changed.
    Query(String),
    /// Typing paused; the number says which keystroke it paused after.
    Debounced(u64),
    /// Puts the keyboard in the search field.
    FocusSearch,
    /// A kind was chosen, by its row in the kinds column.
    Kind(usize),
    /// A source's row in the column under the kinds was selected.
    SourceSelect(usize),
    /// A source's row was opened: its settings are asked for.
    SourceRow(usize),
    /// A sort order was chosen.
    Sort(usize),
    /// A card was selected.
    Select(Grid, usize),
    /// A card was opened.
    Open(Grid, usize),
    /// A card's check was toggled.
    Toggle(Grid, usize),
    /// A home row's whole list was asked for.
    SeeAll(Section),
    /// Back to the previous page.
    Back,
    /// Another source was chosen on an application's page, by its place in the offers.
    Offer(usize),
    /// A failed source is asked again.
    Retry(Source),
    /// A search has run long enough to say so.
    Searching(Generation),
    /// The catalogs on disk were read.
    Loaded(Arc<Loaded>),
    /// pkgstats answered.
    Pkgstats(Result<Arc<[PackageShare]>, Failure>),
    /// Flathub's ranking answered.
    Flathub(Result<Arc<[FlathubInstalls]>, Failure>),
    /// The AUR answered about the packages of the AUR row.
    AurStats(Result<Arc<[AurPackage]>, Failure>),
    /// A source answered a search.
    Found {
        /// The search it answers.
        generation: Generation,
        /// Which source.
        source: Source,
        /// What it found, or why nothing.
        answer: Result<Found, Failure>,
    },
    /// The details of an open application's offer arrived.
    Details {
        /// The card's key.
        key: String,
        /// The offer, by its place.
        offer: usize,
        /// What was learnt, or why nothing.
        answer: Result<Details, Failure>,
    },
    /// Something only the application can do. The store ignores it; the application routes it.
    Request(Request),
}

/// A search and what its sources answered so far.
#[derive(Debug)]
struct Search {
    query: String,
    generation: Generation,
    /// Sources still to answer, in the order they were asked.
    pending: Vec<Source>,
    failed: Vec<(Source, Failure)>,
    repo: Arc<[RepoPackage]>,
    aur: Arc<[AurPackage]>,
    /// Every result, in the order shown.
    ranked: Vec<Card>,
    /// Whether the search has run long enough to show that it is running.
    spinner: bool,
}

impl Search {
    fn settled(&self) -> bool {
        self.pending.is_empty()
    }
}

/// An open application's page.
#[derive(Debug)]
struct OpenApp {
    card: Card,
    /// The offer chosen, by its place.
    offer: usize,
    /// What each offer's details are: `None` while they load.
    details: Vec<Option<Result<Details, Failure>>>,
}

/// The Discover page.
pub struct Store {
    machine: Machine,
    /// The sources the settings keep on, in the order the settings list them.
    enabled: Vec<Source>,
    /// Which sources this machine has, once the application looked.
    sources: Option<Sources>,
    /// The names of the installed pacman packages.
    installed: HashSet<String>,
    featured: Vec<FeaturedApp>,
    loaded: Option<Arc<Loaded>>,
    /// Where each catalog application is in `loaded.apps`, by id key.
    catalog_index: HashMap<String, usize>,
    popularity: Popularity,
    aur_stats: Arc<[AurPackage]>,
    /// The home rows' whole lists, before the kind is applied.
    popular_all: Vec<Card>,
    aur_all: Vec<Card>,
    /// The home rows' lists for the kind chosen, shared with the grids.
    popular: Rc<[Card]>,
    aur_row: Rc<[Card]>,
    /// The search results for the kind chosen, at most [`MAX_RESULTS`].
    results: Rc<[Card]>,
    kind: Kind,
    /// The selected row of the sources under the kinds.
    source_row: Option<usize>,
    query: String,
    typing: u64,
    debounce: Option<TaskId>,
    generations: Generations,
    search: Option<Search>,
    sort: Sort,
    router: Router<Page>,
    selected: HashMap<Grid, String>,
    /// Checked cards, by key, with the offer an install takes.
    checked: Vec<(String, Offer)>,
    open: Option<OpenApp>,
    started: bool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("enabled", &self.enabled)
            .field("query", &self.query)
            .field("kind", &self.kind)
            .field("page", self.router.current())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// The page on `machine`, showing the sources in `enabled`. Nothing is read until
    /// [`init`](Self::init).
    #[must_use]
    pub fn new(machine: Machine, enabled: Vec<Source>) -> Self {
        // The list ships with the program and its tests read it cleanly; a broken entry would be
        // left out, never a failed start.
        let featured = featured::parse(FEATURED).0;
        let mut store = Self {
            machine,
            enabled,
            sources: None,
            installed: HashSet::new(),
            featured,
            loaded: None,
            catalog_index: HashMap::new(),
            popularity: Popularity::default(),
            aur_stats: Arc::from([]),
            popular_all: Vec::new(),
            aur_all: Vec::new(),
            popular: Rc::from([]),
            aur_row: Rc::from([]),
            results: Rc::from([]),
            kind: Kind::All,
            source_row: None,
            query: String::new(),
            typing: 0,
            debounce: None,
            generations: Generations::default(),
            search: None,
            sort: Sort::Relevance,
            router: Router::new(Page::Home),
            selected: HashMap::new(),
            checked: Vec::new(),
            open: None,
            started: false,
        };
        store.rebuild_home();
        store
    }

    /// Starts reading the catalogs and asking the network for its rankings, in the background.
    /// Only the first call does anything.
    pub fn init(&mut self) -> Command<Msg> {
        if self.started {
            return Command::none();
        }
        self.started = true;
        let loader = self.machine.clone();
        let mut commands = vec![Command::perform(move || Msg::Loaded(Arc::new(data::load(&loader))))];
        let runner = Arc::clone(&self.machine.runner);
        commands.push(Command::perform(move || Msg::Pkgstats(data::pkgstats(runner.as_ref()).map(Arc::from))));
        if self.enabled.contains(&Source::Flatpak) {
            let runner = Arc::clone(&self.machine.runner);
            commands
                .push(Command::perform(move || Msg::Flathub(data::flathub_popular(runner.as_ref()).map(Arc::from))));
        }
        let aur_names: Vec<String> = self
            .aur_all
            .iter()
            .flat_map(|card| card.app.offers.iter().filter(|offer| offer.source == Source::Aur))
            .map(|offer| offer.package.clone())
            .collect();
        if !aur_names.is_empty() {
            let runner = Arc::clone(&self.machine.runner);
            commands.push(Command::perform(move || {
                Msg::AurStats(data::aur_info(runner.as_ref(), &aur_names).map(Arc::from))
            }));
        }
        Command::batch(commands)
    }

    /// Tells the page what the machine has: which sources' programs exist and which pacman
    /// packages are installed. Call it after every read of the local database, so "Installed"
    /// marks and the Install and Remove buttons follow what a transaction changed.
    pub fn machine_read(&mut self, sources: &Sources, installed: impl IntoIterator<Item = String>) {
        self.sources = Some(sources.clone());
        self.installed = installed.into_iter().collect();
        self.rebuild_home();
        self.rebuild_results();
        if let Some(open) = &mut self.open {
            let app = open.card.app.clone();
            let votes = open.card.votes;
            open.card = Card::new(app, &self.installed);
            open.card.votes = votes;
        }
    }

    /// Applies a message. [`Msg::Request`] is the application's to route and changes nothing here.
    pub fn update(&mut self, msg: Msg) -> Command<Msg> {
        match msg {
            Msg::Query(text) => return self.query_changed(text),
            Msg::Debounced(typing) if typing == self.typing => return self.start_search(),
            Msg::Debounced(_) | Msg::Request(_) => {}
            Msg::FocusSearch => {
                // The field is on the home page; searching from deeper goes back there first.
                while self.router.back() {}
                self.open = None;
                return Command::focus("store-search");
            }
            Msg::SourceSelect(row) => self.source_row = Some(row),
            Msg::Kind(row) => {
                if let Some(&kind) = self.kinds().get(row) {
                    self.kind = kind;
                    self.rebuild_home();
                    self.rebuild_results();
                }
            }
            Msg::SourceRow(row) => {
                let source = self.enabled.get(row).copied();
                return Command::perform(move || Msg::Request(Request::OpenSettings(source)));
            }
            Msg::Sort(index) => {
                if let Some(&sort) = Sort::ALL.get(index) {
                    self.sort = sort;
                    // A sort asked for reorders at once, settled or not.
                    if let Some(search) = &mut self.search {
                        search.ranked.clear();
                    }
                    self.rebuild_results();
                }
            }
            Msg::Select(grid, index) => {
                if let Some(card) = self.grid_cards(grid).get(index) {
                    self.selected.insert(grid, card.key.clone());
                }
            }
            Msg::Open(grid, index) => {
                if let Some(card) = self.grid_cards(grid).get(index).cloned() {
                    self.selected.insert(grid, card.key.clone());
                    return self.open_app(card);
                }
            }
            Msg::Toggle(grid, index) => {
                if let Some(card) = self.grid_cards(grid).get(index) {
                    self.toggle(&card.clone());
                }
            }
            Msg::SeeAll(section) => {
                self.router.push(Page::Section(section));
                // The keys go on in the list that opened, so arrows and esc work at once.
                return Command::focus("store-all");
            }
            Msg::Back => {
                self.router.back();
                if !matches!(self.router.current(), Page::App(_)) {
                    self.open = None;
                }
            }
            Msg::Offer(index) => return self.choose_offer(index),
            Msg::Retry(source) => return self.retry(source),
            Msg::Searching(generation) => {
                if let Some(search) = &mut self.search
                    && search.generation == generation
                    && !search.settled()
                {
                    search.spinner = true;
                }
            }
            Msg::Loaded(loaded) => {
                self.catalog_index = loaded
                    .apps
                    .iter()
                    .enumerate()
                    .filter_map(|(i, app)| app.id.as_deref().map(|id| (id_key(id), i)))
                    .collect();
                self.loaded = Some(loaded);
                self.rebuild_home();
                // The catalog adds to what a search found; the cards on screen stay in place
                // while sources are still answering.
                self.rebuild_results();
            }
            Msg::Pkgstats(Ok(shares)) => {
                self.popularity.repo_order = shares.iter().map(|share| share.name.clone()).collect();
                self.popularity.repo = shares.iter().map(|share| (share.name.clone(), share.popularity)).collect();
                self.rebuild_home();
            }
            Msg::Flathub(Ok(hits)) => {
                self.popularity.flathub_order = hits.iter().map(|hit| hit.app_id.clone()).collect();
                self.rebuild_home();
            }
            Msg::AurStats(Ok(stats)) => {
                self.aur_stats = stats;
                self.rebuild_home();
            }
            // Without the network the starter list stays; there is nothing to tell.
            Msg::Pkgstats(Err(_)) | Msg::Flathub(Err(_)) | Msg::AurStats(Err(_)) => {}
            Msg::Found { generation, source, answer } => self.found(generation, source, answer),
            Msg::Details { key, offer, answer } => {
                if let Some(open) = &mut self.open
                    && open.card.key == key
                    && let Some(slot) = open.details.get_mut(offer)
                {
                    *slot = Some(answer);
                }
            }
        }
        Command::none()
    }

    /// Uses the sources in `enabled` from now on, after the settings changed. The rows and a
    /// search shown are worked out again; a search does not ask a newly enabled source until the
    /// next one starts.
    pub fn set_enabled(&mut self, enabled: Vec<Source>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.checked.clear();
            self.rebuild_home();
            self.rebuild_results();
        }
    }

    /// Whether cards are checked, so the key that installs them does something.
    #[must_use]
    pub fn has_checks(&self) -> bool {
        !self.checked.is_empty()
    }

    /// Whether a page deeper than the home page is open, so the back key does something.
    #[must_use]
    pub fn can_go_back(&self) -> bool {
        self.router.can_go_back()
    }

    /// Draws the page.
    pub fn view(&self, ui: &mut View<'_, Msg>) {
        view::show(self, ui);
    }

    /// The message for a keymap action while the page is shown: `search` puts the keyboard in
    /// the search field, `back` leaves a section or an application's page, and `install-checked`
    /// asks to install the checked cards. Call it from [`App::action`]; `None` leaves the action
    /// to the application.
    #[must_use]
    pub fn action(&self, name: &str) -> Option<Msg> {
        match name {
            "search" => Some(Msg::FocusSearch),
            "back" if self.router.can_go_back() => Some(Msg::Back),
            "install-checked" if !self.checked.is_empty() => {
                Some(Msg::Request(Request::Install(self.checked_offers())))
            }
            _ => None,
        }
    }

    /// The rows of the kinds column as they stand: `Other` joins while a search finds some.
    fn kinds(&self) -> Vec<Kind> {
        let other = self
            .search
            .as_ref()
            .is_some_and(|search| search.ranked.iter().any(|card| Kind::Other.holds(card.app.category)));
        Kind::column(other || self.kind == Kind::Other)
    }

    /// The cards a grid shows.
    fn grid_cards(&self, grid: Grid) -> Rc<[Card]> {
        match grid {
            Grid::Row(Section::Popular) | Grid::All(Section::Popular) => Rc::clone(&self.popular),
            Grid::Row(Section::Aur) | Grid::All(Section::Aur) => Rc::clone(&self.aur_row),
            Grid::Results => Rc::clone(&self.results),
        }
    }

    /// Whether `source` is on in the settings.
    fn is_enabled(&self, source: Source) -> bool {
        self.enabled.contains(&source)
    }

    fn toggle(&mut self, card: &Card) {
        if let Some(at) = self.checked.iter().position(|(key, _)| *key == card.key) {
            self.checked.remove(at);
        } else if let Some(offer) = card.default_offer() {
            self.checked.push((card.key.clone(), offer.clone()));
        }
    }

    /// The offers the checked cards install.
    fn checked_offers(&self) -> Vec<Offer> {
        self.checked.iter().map(|(_, offer)| offer.clone()).collect()
    }

    /// Works the home rows out again from what is known now.
    fn rebuild_home(&mut self) {
        let empty = Vec::new();
        let apps = self.loaded.as_ref().map_or(&empty, |loaded| &loaded.apps);
        let featured: Vec<_> = self
            .featured
            .iter()
            .filter_map(|entry| {
                model::keep_enabled(model::featured_app(entry, &self.catalog_index, apps), &self.enabled)
            })
            .collect();
        let (aur, popular): (Vec<_>, Vec<_>) = featured.into_iter().partition(model::is_aur_row);
        let installed = &self.installed;
        let ranked = model::ranked_popular(apps, &self.popularity).map(|order| {
            order
                .into_iter()
                .filter_map(|index| model::keep_enabled(apps[index].clone(), &self.enabled))
                .map(|app| Card::new(app, installed))
                .collect::<Vec<_>>()
        });
        self.popular_all = ranked.unwrap_or_else(|| {
            let mut cards: Vec<Card> = popular.into_iter().map(|app| Card::new(app, installed)).collect();
            model::sort_by_share(&mut cards, &self.popularity);
            cards
        });
        self.aur_all = if self.is_enabled(Source::Aur) {
            let mut cards: Vec<Card> = aur.into_iter().map(|app| Card::new(app, installed)).collect();
            model::apply_aur_stats(&mut cards, &self.aur_stats);
            cards
        } else {
            Vec::new()
        };
        let kind = self.kind;
        // Fonts are browsed under their own kind, not among the popular applications.
        let fits = |card: &&Card| match kind {
            Kind::All => card.app.category != qpackages_core::catalog::category::Category::Fonts,
            kind => kind.holds(card.app.category),
        };
        self.popular = self.popular_all.iter().filter(fits).cloned().collect();
        self.aur_row = self.aur_all.iter().filter(fits).cloned().collect();
    }

    fn query_changed(&mut self, text: String) -> Command<Msg> {
        self.query = text;
        self.typing += 1;
        let mut commands = Vec::new();
        if let Some(previous) = self.debounce.take() {
            commands.push(Command::cancel_task(previous));
        }
        if self.query.trim().is_empty() {
            // Clearing the field goes home at once, and whatever is still on its way is dropped.
            let _ = self.generations.start();
            self.search = None;
            self.results = Rc::from([]);
            return Command::batch(commands);
        }
        let typing = self.typing;
        let task = Task::new("debounce", move |cx| {
            if cx.sleep(DEBOUNCE) { Ok(Msg::Debounced(typing)) } else { Err(String::from("typing went on")) }
        });
        self.debounce = Some(task.id());
        commands.push(Command::task(task));
        Command::batch(commands)
    }

    /// Asks every enabled source about the text in the field.
    fn start_search(&mut self) -> Command<Msg> {
        self.debounce = None;
        let generation = self.generations.start();
        let query = self.query.trim().to_owned();
        let mut pending = Vec::new();
        if self.is_enabled(Source::Pacman) {
            pending.push(Source::Pacman);
        }
        if self.is_enabled(Source::Aur) && query.chars().count() >= qpackages_core::catalog::aur::MIN_SEARCH_CHARS {
            pending.push(Source::Aur);
        }
        let commands: Vec<Command<Msg>> = pending.iter().map(|&source| self.ask(source, generation, &query)).collect();
        self.search = Some(Search {
            query,
            generation,
            pending,
            failed: Vec::new(),
            repo: Arc::from([]),
            aur: Arc::from([]),
            ranked: Vec::new(),
            spinner: false,
        });
        self.rebuild_results();
        let spinner = Task::new("searching", move |cx| {
            if cx.sleep(SPINNER_AFTER) { Ok(Msg::Searching(generation)) } else { Err(String::from("cancelled")) }
        });
        Command::batch(commands.into_iter().chain([Command::task(spinner)]))
    }

    /// The background work asking `source` about `query`.
    fn ask(&self, source: Source, generation: Generation, query: &str) -> Command<Msg> {
        let runner = Arc::clone(&self.machine.runner);
        let query = query.to_owned();
        Command::perform(move || {
            let answer = match source {
                Source::Aur => data::search_aur(runner.as_ref(), &query).map(|found| Found::Aur(Arc::from(found))),
                _ => data::search_repo(runner.as_ref(), &query).map(|found| Found::Repo(Arc::from(found))),
            };
            Msg::Found { generation, source, answer }
        })
    }

    fn retry(&mut self, source: Source) -> Command<Msg> {
        let Some(search) = &mut self.search else { return Command::none() };
        let Some(at) = search.failed.iter().position(|(failed, _)| *failed == source) else { return Command::none() };
        search.failed.remove(at);
        search.pending.push(source);
        let (generation, query) = (search.generation, search.query.clone());
        self.ask(source, generation, &query)
    }

    fn found(&mut self, generation: Generation, source: Source, answer: Result<Found, Failure>) {
        // An answer to a search the person has typed past is dropped.
        if !self.generations.is_current(generation) {
            return;
        }
        let Some(search) = &mut self.search else { return };
        search.pending.retain(|&waiting| waiting != source);
        match answer {
            Ok(Found::Repo(packages)) => search.repo = packages,
            Ok(Found::Aur(packages)) => search.aur = packages,
            Err(failure) => search.failed.push((source, failure)),
        }
        self.rebuild_results();
    }

    /// Works the results out again from everything found, keeping cards on screen in place
    /// until every source has answered.
    fn rebuild_results(&mut self) {
        let Some(search) = &mut self.search else {
            self.results = Rc::from([]);
            return;
        };
        let (repo, flathub) = match &self.loaded {
            Some(loaded) => (
                if self.enabled.contains(&Source::Pacman) { loaded.repo.as_slice() } else { &[] },
                if self.enabled.contains(&Source::Flatpak) { loaded.flathub.as_slice() } else { &[] },
            ),
            None => (&[][..], &[][..]),
        };
        let apps: Vec<_> = merge(repo, flathub, &search.repo, &search.aur)
            .into_iter()
            .filter_map(|app| model::keep_enabled(app, &self.enabled))
            .collect();
        let aur_by_name: HashMap<&str, &AurPackage> =
            search.aur.iter().map(|package| (package.name.as_str(), package)).collect();
        let aur_of = |app: &qpackages_core::catalog::merge::App| {
            app.offers
                .iter()
                .find(|offer| offer.source == Source::Aur)
                .and_then(|offer| aur_by_name.get(offer.package.as_str()).copied())
        };
        let popularity = |app: &qpackages_core::catalog::merge::App| {
            self.popularity.of(app).max(aur_of(app).map_or(0.0, |package| package.popularity))
        };
        // Within one kind of match, an application a catalog describes comes before a bare
        // package: it is what a person searching by name means, and without the network the
        // repositories' packages have no popularity to set them apart from AUR builds of them.
        let relevance = |app: &qpackages_core::catalog::merge::App| {
            let catalog = if app.id.is_some() { CATALOG_FIRST } else { 0.0 };
            catalog + popularity(app)
        };
        let index = SearchIndex::from_apps(&apps, relevance);
        let mut cards: Vec<Card> = index
            .search(&search.query)
            .into_iter()
            .map(|hit| {
                let app = &apps[hit.index];
                let mut card = Card::new(app.clone(), &self.installed);
                card.votes = aur_of(app).map(|package| package.votes);
                card
            })
            .collect();
        match self.sort {
            Sort::Relevance => {}
            Sort::Name => cards.sort_by_cached_key(|card| card.app.name.default.to_lowercase()),
            Sort::Popularity => cards.sort_by(|a, b| popularity(&b.app).total_cmp(&popularity(&a.app))),
        }
        search.ranked = if search.settled() || search.ranked.is_empty() {
            cards
        } else {
            let shown: Vec<String> = search.ranked.iter().map(|card| card.key.clone()).collect();
            model::stable_order(&shown, cards)
        };
        let kind = self.kind;
        self.results =
            search.ranked.iter().filter(|card| kind.holds(card.app.category)).take(MAX_RESULTS).cloned().collect();
    }

    fn open_app(&mut self, card: Card) -> Command<Msg> {
        let offers = card.app.offers.len();
        let key = card.key.clone();
        self.open = Some(OpenApp { card, offer: 0, details: vec![None; offers] });
        self.router.push(Page::App(key));
        Command::batch([self.fetch_details(0), Command::focus("store-back")])
    }

    fn choose_offer(&mut self, index: usize) -> Command<Msg> {
        let Some(open) = &mut self.open else { return Command::none() };
        if index >= open.card.app.offers.len() {
            return Command::none();
        }
        open.offer = index;
        if open.details[index].is_some() {
            return Command::none();
        }
        self.fetch_details(index)
    }

    /// Asks for what the card does not know about offer `index` of the open application.
    fn fetch_details(&mut self, index: usize) -> Command<Msg> {
        let Some(open) = &mut self.open else { return Command::none() };
        let Some(offer) = open.card.app.offers.get(index).cloned() else { return Command::none() };
        let key = open.card.key.clone();
        let runner = Arc::clone(&self.machine.runner);
        match offer.source {
            Source::Pacman => Command::perform(move || {
                let answer = data::repo_info(runner.as_ref(), &offer.package).map(|info| Details::Repo(Box::new(info)));
                Msg::Details { key, offer: index, answer }
            }),
            Source::Aur => Command::perform(move || {
                let answer = data::aur_info(runner.as_ref(), std::slice::from_ref(&offer.package)).and_then(|found| {
                    found.into_iter().next().map(|package| Details::Aur(Box::new(package))).ok_or(Failure::Unreadable)
                });
                Msg::Details { key, offer: index, answer }
            }),
            // Flatpak's details are the catalog's, already here; Snap is not read yet.
            Source::Flatpak | Source::Snap => Command::none(),
        }
    }
}

/// The transaction today's flow runs for `request`: an install from the repositories, or a
/// removal, which pacman does for repository and AUR packages alike. Installing from the AUR or
/// Flatpak has no flow yet, so those offers are left out; `None` when nothing is left to run.
#[must_use]
pub fn transaction(request: &Request) -> Option<crate::transaction::Action> {
    let names = |offers: &[Offer], sources: &[Source]| -> Vec<String> {
        offers.iter().filter(|offer| sources.contains(&offer.source)).map(|offer| offer.package.clone()).collect()
    };
    let action = match request {
        Request::Install(offers) => crate::transaction::Action::Install(names(offers, &[Source::Pacman])),
        Request::Remove(offers) => crate::transaction::Action::Remove(names(offers, &[Source::Pacman, Source::Aur])),
        Request::OpenSettings(_) => return None,
    };
    (!action.names().is_empty()).then_some(action)
}

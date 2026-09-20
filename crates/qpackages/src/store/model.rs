//! What the store shows, worked out from what it knows: cards, the kinds they are sorted under,
//! the home page's rows and the order search results stand in.
//!
//! Nothing here reads a file or runs a program; the answers arrive from [`super::data`] and are
//! turned into cards here, so every rule can be tested with plain values.

use std::collections::{HashMap, HashSet};

use qpackages_core::catalog::appstream::Localized;
use qpackages_core::catalog::aur::AurPackage;
use qpackages_core::catalog::category::Category;
use qpackages_core::catalog::featured::FeaturedApp;
use qpackages_core::catalog::flatpak::InstalledApp;
use qpackages_core::catalog::merge::{App, Offer, TRUST_ORDER, id_key};
use qpackages_core::catalog::popularity::FlathubUpdate;
use qpackages_core::snap::api::Snap;
use qpackages_core::sources::Source;

/// The kinds column: everything, one of the browsable kinds, or what none of them holds
/// (libraries and packages nothing placed), which only a search can turn up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Kind {
    /// Every card.
    #[default]
    All,
    /// Cards of one browsable kind.
    Of(Category),
    /// Libraries and cards of no known kind.
    Other,
}

impl Kind {
    /// The rows of the kinds column, in order; `Other` only while a search shows some.
    pub fn column(with_other: bool) -> Vec<Self> {
        let mut kinds = vec![Self::All];
        kinds.extend(Category::BROWSABLE.map(Self::Of));
        if with_other {
            kinds.push(Self::Other);
        }
        kinds
    }

    /// Whether a card of `category` belongs here.
    pub fn holds(self, category: Category) -> bool {
        match self {
            Self::All => true,
            Self::Of(kind) => kind == category,
            Self::Other => !Category::BROWSABLE.contains(&category),
        }
    }

    /// The language key of the kind's short name, for beside a count where the whole name does
    /// not fit.
    pub fn short_key(self) -> String {
        format!("{}-short", self.key())
    }

    /// The language key of the kind's name.
    pub fn key(self) -> String {
        match self {
            Self::All => String::from("store.kind.all"),
            Self::Of(category) => format!("store.kind.{}", category.key()),
            Self::Other => String::from("store.kind.other"),
        }
    }
}

/// What is installed on the machine, as far as the store knows: the pacman packages (the AUR's
/// among them), the Flatpak applications, the latter by id key with every installation that
/// holds them, and the snaps by name.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// The names of the installed pacman packages.
    pub packages: HashSet<String>,
    /// The installed Flatpak applications, by [`id_key`] of their id: one entry for each
    /// installation that holds the application, with the id as Flatpak spells it.
    pub flatpaks: HashMap<String, Vec<InstalledApp>>,
    /// The installed snaps by name, the bases and snapd itself among them: a base is not a card,
    /// but it is installed, and removing an application never takes its base away.
    pub snaps: HashMap<String, Snap>,
}

impl Installed {
    /// Takes `apps` as the installed Flatpak applications, replacing what was known.
    pub fn set_flatpaks(&mut self, apps: &[InstalledApp]) {
        self.flatpaks.clear();
        for app in apps {
            self.flatpaks.entry(id_key(&app.app_id)).or_default().push(app.clone());
        }
    }

    /// Takes `snaps` as the installed snaps, replacing what was known.
    pub fn set_snaps(&mut self, snaps: &[Snap]) {
        self.snaps.clear();
        for snap in snaps {
            self.snaps.insert(snap.name.clone(), snap.clone());
        }
    }

    /// Whether `offer` is installed: its package for pacman and the AUR, its application id for
    /// Flatpak, its name for Snap. A Flatpak id is never taken for a package name, nor the other
    /// way round, and a snap name is compared as snapd spells it: snap names are lower case
    /// already, so there is nothing to fold.
    pub fn has(&self, offer: &Offer) -> bool {
        match offer.source {
            Source::Pacman | Source::Aur => self.packages.contains(&offer.package),
            Source::Flatpak => self.flatpaks.contains_key(&id_key(&offer.package)),
            Source::Snap => self.snaps.contains_key(&offer.package),
        }
    }
}

/// One card: an application, whether it is installed and, for the AUR, its votes.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// What the card is known by across rebuilds: the AppStream id, or its first offer.
    pub key: String,
    /// The application and every source that offers it.
    pub app: App,
    /// The AUR's vote count, once the AUR said.
    pub votes: Option<u64>,
    /// Whether one of its packages is installed.
    pub installed: bool,
}

impl Card {
    /// A card for `app`, installed when `installed` has one of its offers.
    pub fn new(app: App, installed: &Installed) -> Self {
        let is_installed = app.offers.iter().any(|offer| installed.has(offer));
        Self { key: key(&app), app, votes: None, installed: is_installed }
    }

    /// The name in the language shown.
    pub fn name(&self, turkish: bool) -> &str {
        self.app.name.get(turkish)
    }

    /// The summary in the language shown, if there is one.
    pub fn summary(&self, turkish: bool) -> Option<&str> {
        self.app.summary.as_ref().map(|summary| summary.get(turkish))
    }

    /// The name the icon table is asked for: the repository package where there is one, since
    /// the table is keyed by package names, then the AppStream id, then any package.
    pub fn icon_name(&self) -> &str {
        self.app
            .offers
            .iter()
            .find(|offer| offer.source == Source::Pacman)
            .map(|offer| offer.package.as_str())
            .or(self.app.id.as_deref())
            .or_else(|| self.app.offers.first().map(|offer| offer.package.as_str()))
            .unwrap_or_default()
    }

    /// The source the icon falls back to: the most trusted one offering it.
    pub fn first_source(&self) -> Source {
        self.app.offers.first().map_or(Source::Pacman, |offer| offer.source)
    }

    /// The offer an install takes when nobody chose: the most trusted.
    pub fn default_offer(&self) -> Option<&Offer> {
        self.app.offers.first()
    }
}

/// The key of an application: its AppStream id compared the way the merge compares them, or
/// `<source>:<package>` of its first offer for a package no catalog knows.
pub fn key(app: &App) -> String {
    match (&app.id, app.offers.first()) {
        (Some(id), _) => id_key(id),
        (None, Some(offer)) => format!("{}:{}", crate::sources::name(offer.source), offer.package),
        (None, None) => app.name.default.clone(),
    }
}

/// Keeps only the offers of `enabled` sources, most trusted first. An application no enabled
/// source offers is gone.
pub fn keep_enabled(mut app: App, enabled: &[Source]) -> Option<App> {
    app.offers.retain(|offer| enabled.contains(&offer.source));
    app.sort_offers(&TRUST_ORDER);
    (!app.offers.is_empty()).then_some(app)
}

/// A starter-list entry as an application: the catalog's record when it knows the id, with the
/// list's offers added to it, otherwise what the list itself says.
pub fn featured_app(entry: &FeaturedApp, catalog: &HashMap<String, usize>, apps: &[App]) -> App {
    let known = entry.id.as_deref().and_then(|id| catalog.get(&id_key(id))).map(|&index| &apps[index]);
    let mut app = known.cloned().unwrap_or_else(|| App {
        id: entry.id.clone(),
        name: Localized { default: entry.name.clone(), turkish: None },
        summary: entry.summary.clone(),
        category: entry.category,
        icon: None,
        keywords: Vec::new(),
        keywords_turkish: Vec::new(),
        offers: Vec::new(),
    });
    if app.summary.is_none() {
        app.summary.clone_from(&entry.summary);
    }
    for offer in &entry.offers {
        if !app.offers.contains(offer) {
            app.offers.push(offer.clone());
        }
    }
    app.sort_offers(&TRUST_ORDER);
    app
}

/// A Flathub update as an application: the catalog's record when it knows the id, otherwise
/// what Flathub itself says, offered as a Flatpak.
pub fn recent_app(update: &FlathubUpdate, catalog: &HashMap<String, usize>, apps: &[App]) -> App {
    let offer = Offer { source: Source::Flatpak, package: update.app_id.clone() };
    if let Some(&index) = catalog.get(&id_key(&update.app_id)) {
        let mut app = apps[index].clone();
        if !app.offered_by(Source::Flatpak) {
            app.offers.push(offer);
            app.sort_offers(&TRUST_ORDER);
        }
        return app;
    }
    App {
        id: Some(update.app_id.clone()),
        name: Localized { default: update.name.clone(), turkish: None },
        summary: update.summary.clone().map(|default| Localized { default, turkish: None }),
        category: update.main_category.as_deref().and_then(Category::from_flathub).unwrap_or(Category::Unknown),
        icon: None,
        keywords: Vec::new(),
        keywords_turkish: Vec::new(),
        offers: vec![offer],
    }
}

/// A snap as an application of its own.
///
/// A snap never joins a repository or Flathub entry: it carries no AppStream id, and its name is
/// its own — the `hello` snap is GNU Hello while the `hello` package is something else — so nothing
/// but the name could be matched on, and matching on it would merge applications that are not the
/// same. Its card therefore stands alone, the way an AUR-only package's does.
///
/// It has no kind either: snapd's search answer carries no category for a snap, so the card falls
/// under what none of the kinds holds and is found by searching.
pub fn snap_app(snap: &Snap) -> App {
    App {
        id: None,
        name: Localized { default: snap.title.clone(), turkish: None },
        summary: snap.summary.clone().map(|default| Localized { default, turkish: None }),
        category: Category::Unknown,
        icon: None,
        // The name is worth searching by as well: a person looking for `hello-world` types the
        // name, not the title the store shows.
        keywords: vec![snap.name.clone()],
        keywords_turkish: Vec::new(),
        offers: vec![Offer { source: Source::Snap, package: snap.name.clone() }],
    }
}

/// Which home row a starter entry belongs to. The AUR row holds what only the AUR (and maybe
/// Flathub) has; anything the repositories offer is a popular application, the way the design
/// draws Firefox there and Chrome in the AUR row.
pub fn is_aur_row(app: &App) -> bool {
    app.offered_by(Source::Aur) && !app.offered_by(Source::Pacman)
}

/// How widely the repository packages are used, from pkgstats, and how often Flathub
/// applications were installed last month.
#[derive(Debug, Clone, Default)]
pub struct Popularity {
    /// Package name to its share of reporting machines, in percent.
    pub repo: HashMap<String, f64>,
    /// Package names, most used first, as pkgstats sent them.
    pub repo_order: Vec<String>,
    /// Flathub ids, most installed first.
    pub flathub_order: Vec<String>,
}

impl Popularity {
    /// A number to break ties in search with: the pkgstats share of its repository package, else
    /// nothing. The AUR's own popularity is given by the caller, which has it.
    pub fn of(&self, app: &App) -> f64 {
        app.offers
            .iter()
            .filter(|offer| offer.source == Source::Pacman)
            .filter_map(|offer| self.repo.get(&offer.package))
            .fold(0.0, |best: f64, share| best.max(*share))
    }
}

/// The "popular applications" list once the network answered: pkgstats' order kept to the
/// catalog's applications from the repositories, then Flathub's order kept to its own
/// applications. `None` while neither ranking or no catalog is there.
pub fn ranked_popular(apps: &[App], popularity: &Popularity) -> Option<Vec<usize>> {
    if apps.is_empty() || (popularity.repo_order.is_empty() && popularity.flathub_order.is_empty()) {
        return None;
    }
    let mut by_package: HashMap<&str, usize> = HashMap::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (index, app) in apps.iter().enumerate() {
        for offer in &app.offers {
            match offer.source {
                Source::Pacman => {
                    by_package.entry(offer.package.as_str()).or_insert(index);
                }
                Source::Flatpak => {
                    by_id.entry(id_key(&offer.package)).or_insert(index);
                }
                // Neither ranking knows about snaps: pkgstats counts packages and Flathub its own.
                Source::Aur | Source::Snap => {}
            }
        }
    }
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    let repo = popularity.repo_order.iter().filter_map(|name| by_package.get(name.as_str()).copied());
    let flathub = popularity.flathub_order.iter().filter_map(|id| by_id.get(&id_key(id)).copied());
    for index in repo.chain(flathub) {
        if seen.insert(index) {
            order.push(index);
        }
    }
    (!order.is_empty()).then_some(order)
}

/// Orders the starter entries by pkgstats when no catalog is there to rank from: the network
/// still says which are used most. Entries pkgstats does not know keep their place at the end.
pub fn sort_by_share(cards: &mut [Card], popularity: &Popularity) {
    cards.sort_by(|a, b| popularity.of(&b.app).total_cmp(&popularity.of(&a.app)));
}

/// Orders AUR cards by the RPC's popularity, most popular first, and fills in their votes.
pub fn apply_aur_stats(cards: &mut [Card], stats: &[AurPackage]) {
    let by_name: HashMap<&str, &AurPackage> = stats.iter().map(|package| (package.name.as_str(), package)).collect();
    let aur_of = |card: &Card| {
        card.app
            .offers
            .iter()
            .find(|offer| offer.source == Source::Aur)
            .and_then(|offer| by_name.get(offer.package.as_str()).copied())
    };
    for card in cards.iter_mut() {
        if let Some(package) = aur_of(card) {
            card.votes = Some(package.votes);
        }
    }
    let popularity = |card: &Card| aur_of(card).map_or(-1.0, |package| package.popularity);
    cards.sort_by(|a, b| popularity(b).total_cmp(&popularity(a)));
}

/// The order a changing result set is shown in while sources are still answering.
///
/// `ranked` is the order the results would take if they were complete. Cards already on screen
/// keep their order among themselves: they fill, in their old order, the places the ranking
/// gives to old cards, and new cards take the places the ranking gives them. Nothing on screen
/// jumps past another card; new ones appear where they will stay.
pub fn stable_order(shown: &[String], ranked: Vec<Card>) -> Vec<Card> {
    let old: HashSet<&str> = shown.iter().map(String::as_str).collect();
    let mut by_key: HashMap<String, Card> = HashMap::new();
    let mut slots: Vec<Option<String>> = Vec::with_capacity(ranked.len());
    for card in ranked {
        slots.push((!old.contains(card.key.as_str())).then(|| card.key.clone()));
        by_key.insert(card.key.clone(), card);
    }
    let kept: Vec<String> = shown.iter().filter(|key| by_key.contains_key(key.as_str())).cloned().collect();
    let mut kept = kept.into_iter();
    slots
        .into_iter()
        .filter_map(|slot| match slot {
            Some(new) => by_key.remove(&new),
            None => kept.next().and_then(|key| by_key.remove(&key)),
        })
        .collect()
}

/// How search results are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Best match first, as the search ranks them.
    #[default]
    Relevance,
    /// By name.
    Name,
    /// Most used first.
    Popularity,
}

impl Sort {
    /// Every order, as the sort menu lists them.
    pub const ALL: [Self; 3] = [Self::Relevance, Self::Name, Self::Popularity];

    /// The language key of its name.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Relevance => "store.sort.relevance",
            Self::Name => "store.sort.name",
            Self::Popularity => "store.sort.popularity",
        }
    }
}

/// How many results each kind holds, for the counts beside the kinds.
pub fn kind_counts(cards: &[Card]) -> HashMap<Kind, usize> {
    let mut counts = HashMap::new();
    for card in cards {
        *counts.entry(Kind::All).or_insert(0) += 1;
        let kind =
            if Category::BROWSABLE.contains(&card.app.category) { Kind::Of(card.app.category) } else { Kind::Other };
        *counts.entry(kind).or_insert(0) += 1;
    }
    counts
}

/// A vote count the way a card has room for it: `312`, `4.2k`, `12k`.
pub fn compact(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..10_000 => {
            let tenths = (count + 50) / 100;
            if tenths.is_multiple_of(10) {
                format!("{}k", tenths / 10)
            } else {
                format!("{}.{}k", tenths / 10, tenths % 10)
            }
        }
        _ => format!("{}k", (count + 500) / 1000),
    }
}

#[cfg(test)]
mod tests {
    use qpackages_core::catalog::flatpak::Installation;

    use super::*;

    fn app(name: &str, offers: &[(Source, &str)]) -> App {
        App {
            id: None,
            name: Localized { default: name.to_owned(), turkish: None },
            summary: None,
            category: Category::Unknown,
            icon: None,
            keywords: vec![],
            keywords_turkish: vec![],
            offers: offers
                .iter()
                .map(|(source, package)| Offer { source: *source, package: (*package).to_owned() })
                .collect(),
        }
    }

    fn card(name: &str) -> Card {
        Card::new(app(name, &[(Source::Pacman, name)]), &Installed::default())
    }

    fn keys(cards: &[Card]) -> Vec<&str> {
        cards.iter().map(|card| card.key.as_str()).collect()
    }

    #[test]
    fn old_cards_keep_their_order_and_new_ones_take_their_ranked_place() {
        let shown = [String::from("pacman:b"), String::from("pacman:a")];
        let ranked = ["a", "c", "b", "d"].map(card).to_vec();
        let order = stable_order(&shown, ranked);
        assert_eq!(keys(&order), ["pacman:b", "pacman:c", "pacman:a", "pacman:d"]);
    }

    #[test]
    fn a_card_that_left_the_ranking_leaves_the_screen() {
        let shown = [String::from("pacman:gone"), String::from("pacman:a")];
        let order = stable_order(&shown, vec![card("a")]);
        assert_eq!(keys(&order), ["pacman:a"]);
    }

    #[test]
    fn installed_is_about_packages_and_flatpak_ids_apart() {
        let mut installed = Installed { packages: HashSet::from([String::from("firefox")]), ..Installed::default() };
        let firefox = Card::new(app("Firefox", &[(Source::Pacman, "firefox")]), &installed);
        assert!(firefox.installed);
        let flatpak = Card::new(app("Firefox", &[(Source::Flatpak, "firefox")]), &installed);
        assert!(!flatpak.installed, "a Flatpak id is not a package name");
        installed.set_flatpaks(&[InstalledApp {
            app_id: String::from("org.videolan.VLC"),
            installation: Installation::User,
        }]);
        let vlc = Card::new(app("VLC", &[(Source::Pacman, "vlc"), (Source::Flatpak, "org.videolan.vlc")]), &installed);
        assert!(vlc.installed, "ids compare without case");
        assert!(!installed.has(&Offer { source: Source::Pacman, package: String::from("org.videolan.VLC") }));
    }

    #[test]
    fn keys_follow_the_id_or_the_first_offer() {
        let mut with_id = app("VLC", &[(Source::Pacman, "vlc")]);
        with_id.id = Some(String::from("org.videolan.VLC"));
        assert_eq!(key(&with_id), "org.videolan.vlc");
        assert_eq!(key(&app("x", &[(Source::Aur, "x-bin")])), "aur:x-bin");
    }

    #[test]
    fn disabled_sources_drop_their_offers_and_an_app_without_offers_is_gone() {
        let both = app("Spotify", &[(Source::Aur, "spotify"), (Source::Flatpak, "com.spotify.Client")]);
        let kept = keep_enabled(both.clone(), &[Source::Pacman, Source::Flatpak]).expect("Flatpak offers it");
        assert_eq!(kept.offers.len(), 1);
        assert!(keep_enabled(both, &[Source::Pacman]).is_none());
    }

    #[test]
    fn the_popular_ranking_keeps_to_applications_and_joins_both_orders() {
        let mut gimp = app("GIMP", &[(Source::Pacman, "gimp"), (Source::Flatpak, "org.gimp.GIMP")]);
        gimp.id = Some(String::from("org.gimp.GIMP"));
        let spotify = app("Spotify", &[(Source::Flatpak, "com.spotify.Client")]);
        let firefox = app("Firefox", &[(Source::Pacman, "firefox")]);
        let apps = [gimp, spotify, firefox];
        let popularity = Popularity {
            repo_order: ["bash", "firefox", "gimp"].map(String::from).to_vec(),
            flathub_order: ["org.gimp.GIMP", "com.spotify.Client"].map(String::from).to_vec(),
            ..Popularity::default()
        };
        assert_eq!(ranked_popular(&apps, &popularity), Some(vec![2, 0, 1]), "bash is no application; GIMP once");
        assert_eq!(ranked_popular(&[], &popularity), None);
        assert_eq!(ranked_popular(&apps, &Popularity::default()), None);
    }

    #[test]
    fn aur_cards_take_their_votes_and_the_most_popular_leads() {
        let mut cards = [("a", 1.0, 10), ("b", 5.0, 4_200)]
            .map(|(name, _, _)| Card::new(app(name, &[(Source::Aur, name)]), &Installed::default()))
            .to_vec();
        let stats = [("a", 1.0, 10), ("b", 5.0, 4_200)].map(|(name, popularity, votes)| AurPackage {
            name: name.to_owned(),
            popularity,
            votes,
            ..AurPackage::default()
        });
        apply_aur_stats(&mut cards, &stats);
        assert_eq!(keys(&cards), ["aur:b", "aur:a"]);
        assert_eq!(cards[0].votes, Some(4_200));
    }

    #[test]
    fn a_flathub_update_takes_the_catalogs_record_or_stands_on_its_own() {
        let update = |id: &str, category: Option<&str>| FlathubUpdate {
            app_id: id.to_owned(),
            name: String::from("Name from Flathub"),
            summary: Some(String::from("Summary from Flathub")),
            main_category: category.map(str::to_owned),
            updated_at: Some(1),
        };
        let mut gimp = app("GIMP", &[(Source::Pacman, "gimp")]);
        gimp.id = Some(String::from("org.gimp.GIMP"));
        gimp.name.turkish = Some(String::from("GIMP tr"));
        let catalog = HashMap::from([(id_key("org.gimp.GIMP"), 0)]);
        let known = recent_app(&update("org.gimp.GIMP", Some("graphics")), &catalog, &[gimp]);
        assert_eq!(known.name.turkish.as_deref(), Some("GIMP tr"), "the catalog's names and translations");
        let sources: Vec<Source> = known.offers.iter().map(|offer| offer.source).collect();
        assert_eq!(sources, [Source::Pacman, Source::Flatpak], "and Flathub joins its offers");
        let unknown = recent_app(&update("io.example.New", Some("audiovideo")), &HashMap::new(), &[]);
        assert_eq!(unknown.name.default, "Name from Flathub");
        assert_eq!(unknown.category, Category::AudioVideo);
        assert_eq!(unknown.offers, [Offer { source: Source::Flatpak, package: String::from("io.example.New") }]);
        assert_eq!(recent_app(&update("x.y.Z", Some("education")), &HashMap::new(), &[]).category, Category::Unknown);
    }

    #[test]
    fn counts_are_compact() {
        assert_eq!(compact(312), "312");
        assert_eq!(compact(4_200), "4.2k");
        assert_eq!(compact(1_708), "1.7k");
        assert_eq!(compact(2_000), "2k");
        assert_eq!(compact(9_990), "10k");
        assert_eq!(compact(12_400), "12k");
    }

    #[test]
    fn kinds_count_every_card_once_and_other_takes_the_rest() {
        let mut firefox = card("firefox");
        firefox.app.category = Category::Internet;
        let counts = kind_counts(&[firefox, card("libfoo")]);
        assert_eq!(counts[&Kind::All], 2);
        assert_eq!(counts[&Kind::Of(Category::Internet)], 1);
        assert_eq!(counts[&Kind::Other], 1);
        assert!(Kind::Other.holds(Category::Library));
        assert!(!Kind::Other.holds(Category::Game));
        assert_eq!(Kind::column(false).len(), 10);
    }
}

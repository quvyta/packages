//! Finding entries as the person types.
//!
//! Every searchable text is lowercased once, when the index is built, so a keystroke only
//! lowercases the query. Results are ranked by how the query matched: the whole name, the start
//! of a name, anywhere in a name, then anywhere in a summary or keyword. Within one rank the more
//! popular entry comes first.
//!
//! Sources answer at different speeds and a person types faster than the AUR answers, so each
//! search gets a [`Generation`]; an answer that arrives for an older one is dropped.

use super::merge::App;

/// How a query matched an entry, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rank {
    /// A name is exactly the query.
    ExactName,
    /// A name starts with the query.
    NamePrefix,
    /// A name contains the query.
    NameContains,
    /// A summary or keyword contains the query.
    TextContains,
}

/// One match: which entry, and how it matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// The entry's position, in the order entries were added.
    pub index: usize,
    /// How it matched.
    pub rank: Rank,
}

/// One entry's texts, lowercased.
#[derive(Debug, Clone)]
struct Entry {
    names: Vec<String>,
    texts: Vec<String>,
    popularity: f64,
}

/// A lowercased copy of every entry's names and texts, ready to search.
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    entries: Vec<Entry>,
}

impl SearchIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry and returns its index. `names` are what the entry is called (its name in
    /// each language, its package names); `texts` are its summaries and keywords. `popularity`
    /// breaks ties within a rank, higher first; any scale works as long as it is the same for
    /// every entry.
    pub fn add<N: AsRef<str>, T: AsRef<str>>(&mut self, names: &[N], texts: &[T], popularity: f64) -> usize {
        let names = names.iter().map(|name| name.as_ref().to_lowercase()).collect();
        let texts = texts.iter().map(|text| text.as_ref().to_lowercase()).collect();
        self.entries.push(Entry { names, texts, popularity });
        self.entries.len() - 1
    }

    /// An index of merged applications, in their order: each is found by its names in both
    /// languages and its package names, and by its summaries and keywords. `popularity` gives
    /// each application's standing.
    #[must_use]
    pub fn from_apps(apps: &[App], popularity: impl Fn(&App) -> f64) -> Self {
        let mut index = Self::new();
        for app in apps {
            let mut names: Vec<&str> = vec![&app.name.default];
            names.extend(app.name.turkish.as_deref());
            names.extend(app.offers.iter().map(|offer| offer.package.as_str()));
            let mut texts: Vec<&str> = Vec::new();
            if let Some(summary) = &app.summary {
                texts.push(&summary.default);
                texts.extend(summary.turkish.as_deref());
            }
            texts.extend(app.keywords.iter().chain(&app.keywords_turkish).map(String::as_str));
            index.add(&names, &texts, popularity(app));
        }
        index
    }

    /// How many entries the index holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry `query` matches, best first. Surrounding whitespace is ignored and an empty
    /// query matches nothing: with nothing typed the store shows its home page, not a list.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<Hit> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<Hit> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| rank(entry, &query).map(|rank| Hit { index, rank }))
            .collect();
        // The sort is stable, so entries equal in rank and popularity keep the order they were added.
        hits.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| self.entries[b.index].popularity.total_cmp(&self.entries[a.index].popularity))
        });
        hits
    }
}

/// How `query`, already lowercased, matches `entry`, if at all.
fn rank(entry: &Entry, query: &str) -> Option<Rank> {
    let best_name = entry
        .names
        .iter()
        .filter_map(|name| {
            if name == query {
                Some(Rank::ExactName)
            } else if name.starts_with(query) {
                Some(Rank::NamePrefix)
            } else {
                name.contains(query).then_some(Rank::NameContains)
            }
        })
        .min();
    best_name.or_else(|| entry.texts.iter().any(|text| text.contains(query)).then_some(Rank::TextContains))
}

/// Which search an answer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

/// Numbers searches so answers to old ones can be told apart.
#[derive(Debug, Clone, Default)]
pub struct Generations {
    latest: u64,
}

impl Generations {
    /// Starts a new search; every earlier generation is now stale.
    pub fn start(&mut self) -> Generation {
        self.latest += 1;
        Generation(self.latest)
    }

    /// Whether an answer for `generation` still belongs on screen.
    #[must_use]
    pub const fn is_current(&self, generation: Generation) -> bool {
        generation.0 == self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::merge::{RepoPackage, merge};
    use crate::catalog::{appstream, aur};

    fn index(entries: &[(&str, &str, f64)]) -> SearchIndex {
        let mut index = SearchIndex::new();
        for (name, summary, popularity) in entries {
            index.add(&[name], &[summary], *popularity);
        }
        index
    }

    fn ranked(index: &SearchIndex, query: &str) -> Vec<(usize, Rank)> {
        index.search(query).into_iter().map(|hit| (hit.index, hit.rank)).collect()
    }

    #[test]
    fn exact_then_prefix_then_contains_then_summary() {
        let index = index(&[
            ("obs-vkcapture", "Vulkan game capture", 1.0),
            ("libobs", "Library", 50.0),
            ("obs", "Streaming", 0.0),
            ("kdenlive", "Video editor, works with OBS recordings", 99.0),
            ("firefox", "Web browser", 99.0),
        ]);
        assert_eq!(
            ranked(&index, "obs"),
            [(2, Rank::ExactName), (0, Rank::NamePrefix), (1, Rank::NameContains), (3, Rank::TextContains)],
            "popularity never lifts an entry above a better match"
        );
    }

    #[test]
    fn popularity_breaks_ties_within_a_rank() {
        let index = index(&[("obs-a", "", 1.0), ("obs-b", "", 30.0), ("obs-c", "", 5.0)]);
        assert_eq!(ranked(&index, "obs").iter().map(|(index, _)| *index).collect::<Vec<_>>(), [1, 2, 0]);
    }

    #[test]
    fn case_and_surrounding_space_do_not_matter() {
        let index = index(&[("OBS Studio", "", 0.0)]);
        assert_eq!(ranked(&index, "  obs STUDIO "), [(0, Rank::ExactName)]);
        assert!(index.search("   ").is_empty());
        assert!(index.search("").is_empty());
    }

    #[test]
    fn turkish_text_is_searchable() {
        let index = index(&[("GIMP", "Görüntü düzenleyici", 0.0)]);
        assert_eq!(ranked(&index, "GÖRÜNTÜ"), [(0, Rank::TextContains)]);
    }

    #[test]
    fn searches_the_merged_recorded_catalog() {
        let repo = appstream::parse(include_str!("../../tests/fixtures/catalog/arch-extra.xml")).0.components;
        let flathub = appstream::parse(include_str!("../../tests/fixtures/catalog/flathub.xml")).0.components;
        let aur =
            aur::parse_response(include_str!("../../tests/fixtures/catalog/aur-search-obs.json")).expect("recorded");
        let apps = merge(&repo, &flathub, &[RepoPackage::default()], &aur);
        let index = SearchIndex::from_apps(&apps, |app| app.offers.len() as f64);
        assert_eq!(index.len(), apps.len());

        let hits = index.search("obs studio");
        assert_eq!(apps[hits[0].index].id.as_deref(), Some("com.obsproject.Studio"));
        assert_eq!(hits[0].rank, Rank::ExactName);

        let hits = index.search("tarayıcı");
        assert!(
            hits.iter().any(|hit| apps[hit.index].id.as_deref() == Some("org.mozilla.firefox")),
            "a Turkish keyword"
        );

        let hits = index.search("obs-studio-git");
        assert_eq!(hits[0].rank, Rank::ExactName, "package names count as names");
        assert_eq!(apps[hits[0].index].id.as_deref(), Some("com.obsproject.Studio"), "the AUR build joined the card");
    }

    #[test]
    fn answers_to_an_older_search_are_stale() {
        let mut generations = Generations::default();
        let first = generations.start();
        assert!(generations.is_current(first));
        let second = generations.start();
        assert!(!generations.is_current(first));
        assert!(generations.is_current(second));
        assert!(first < second);
    }
}

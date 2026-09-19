//! What the user typed in the Installed tab's search: words to look for, and optionally which
//! source the packages must come from.
//!
//! `source:aur` keeps the packages no repository offers and `source:repo` (or `source:pacman`)
//! those a repository does; `kaynak:` is the same word in Turkish, so neither language's users
//! have to remember the other's. The filter may stand anywhere among the words.

use qpackages_core::pacman::Package;

use super::table::Origin;

/// The prefixes that turn a word into a source filter.
const PREFIXES: [&str; 2] = ["source:", "kaynak:"];

/// A parsed search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// The words that are not a filter, lower case and joined by single spaces.
    text: String,
    /// The source asked for, if any.
    source: Option<Wanted>,
}

/// The source a filter asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    /// A source that exists.
    Is(Origin),
    /// A name no source has, which nothing matches rather than being ignored: a typo should show
    /// an empty table, not every package.
    Nothing,
}

impl Query {
    /// Reads `search`.
    #[must_use]
    pub fn parse(search: &str) -> Self {
        let mut words = Vec::new();
        let mut source = None;
        for word in search.split_whitespace() {
            let lower = word.to_lowercase();
            match PREFIXES.iter().find_map(|prefix| lower.strip_prefix(prefix)) {
                Some(value) => source = Some(wanted(value)),
                None => words.push(lower),
            }
        }
        Self { text: words.join(" "), source }
    }

    /// Whether `package`, which came from `origin` when that is known, is what the search asks
    /// for: a case-insensitive part of its name or of its description, the way `pacman -Qs`
    /// looks, from the source asked for. An empty search matches everything.
    #[must_use]
    pub fn matches(&self, package: &Package, origin: Option<Origin>) -> bool {
        let source = match self.source {
            None => true,
            Some(Wanted::Is(wanted)) => origin == Some(wanted),
            Some(Wanted::Nothing) => false,
        };
        source
            && (self.text.is_empty()
                || package.name.to_lowercase().contains(&self.text)
                || package.description.as_deref().is_some_and(|text| text.to_lowercase().contains(&self.text)))
    }
}

/// The source a filter's value names.
fn wanted(value: &str) -> Wanted {
    match value {
        "aur" => Wanted::Is(Origin::Aur),
        "repo" | "pacman" | "depo" => Wanted::Is(Origin::Repo),
        _ => Wanted::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, description: &str) -> Package {
        Package { name: name.to_owned(), description: Some(description.to_owned()), ..Package::default() }
    }

    #[test]
    fn words_match_the_name_or_the_description_in_any_case() {
        let bash = package("bash", "The GNU Bourne Again shell");
        assert!(Query::parse("").matches(&bash, None));
        assert!(Query::parse("BA").matches(&bash, None));
        assert!(Query::parse("  bourne   again ").matches(&bash, None), "spaces are folded");
        assert!(!Query::parse("zsh").matches(&bash, None));
    }

    #[test]
    fn a_source_filter_keeps_the_packages_of_that_source_wherever_it_stands() {
        let paru = package("paru", "Feature packed AUR helper");
        assert!(Query::parse("source:aur").matches(&paru, Some(Origin::Aur)));
        assert!(Query::parse("par SOURCE:AUR").matches(&paru, Some(Origin::Aur)));
        assert!(Query::parse("kaynak:aur par").matches(&paru, Some(Origin::Aur)), "the Turkish word works too");
        assert!(!Query::parse("source:aur").matches(&paru, Some(Origin::Repo)));
        assert!(Query::parse("source:repo").matches(&paru, Some(Origin::Repo)));
        assert!(Query::parse("kaynak:depo").matches(&paru, Some(Origin::Repo)));
        assert!(!Query::parse("source:aur").matches(&paru, None), "an unknown source is not claimed");
        assert!(!Query::parse("source:aur zsh").matches(&paru, Some(Origin::Aur)), "the words still count");
    }

    #[test]
    fn a_source_nobody_has_matches_nothing() {
        let bash = package("bash", "");
        assert!(!Query::parse("source:brew").matches(&bash, Some(Origin::Repo)));
        assert!(!Query::parse("source:").matches(&bash, Some(Origin::Repo)));
    }
}

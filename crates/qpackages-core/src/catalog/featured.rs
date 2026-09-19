//! The starter list: applications shown before the network answers, or when it never does.
//!
//! The list ships inside the application as a small TOML file of `[[app]]` tables:
//!
//! ```toml
//! [[app]]
//! name = "Firefox"
//! id = "org.mozilla.firefox"
//! category = "internet"
//! sources = ["pacman:firefox", "flatpak:org.mozilla.firefox"]
//! ```
//!
//! `name` is shown on the card. `id`, the AppStream id, is optional; with it the entry is matched
//! to the catalog's record for its summary and icon. `category` is a [`Category::key`]. Each of
//! `sources` is `<source>:<package>`, the source one of `pacman`, `flatpak`, `snap` and `aur`.
//! An entry with a problem is skipped and reported; the rest of the list survives.

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::Problem;
use super::category::Category;
use super::merge::{Offer, TRUST_ORDER};
use crate::sources::Source;

/// The keys an `[[app]]` table may have.
const KEYS: [&str; 4] = ["name", "id", "category", "sources"];

/// One application of the starter list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeaturedApp {
    /// The name on the card.
    pub name: String,
    /// The AppStream id, when the list gives one.
    pub id: Option<String>,
    /// The kind it is sorted under.
    pub category: Category,
    /// Where it can be installed from, most trusted first.
    pub offers: Vec<Offer>,
}

/// Reads a starter list. Returns the entries that could be read, in file order, and every
/// problem met.
#[must_use]
pub fn parse(text: &str) -> (Vec<FeaturedApp>, Vec<Problem>) {
    let root = match DeTable::parse(text) {
        Ok(root) => root.into_inner(),
        Err(error) => {
            let offset = error.span().map_or(0, |span| span.start);
            return (Vec::new(), vec![Problem::at(text, offset, error.message())]);
        }
    };
    let mut apps = Vec::new();
    let mut problems = Vec::new();
    for (key, value) in &root {
        if key.get_ref().as_ref() != "app" {
            problems.push(Problem::at(
                text,
                key.span().start,
                format!("unknown key `{}`; expected [[app]]", key.get_ref()),
            ));
            continue;
        }
        let Some(entries) = value.get_ref().as_array() else {
            problems.push(Problem::at(text, value.span().start, "`app` must be a list of [[app]] tables"));
            continue;
        };
        for entry in entries {
            match read_app(entry) {
                Ok(app) => apps.push(app),
                Err((offset, message)) => problems.push(Problem::at(text, offset, message)),
            }
        }
    }
    (apps, problems)
}

/// One `[[app]]` table, or where and why it cannot be read.
fn read_app(entry: &Spanned<DeValue<'_>>) -> Result<FeaturedApp, (usize, String)> {
    let at = |value: &Spanned<DeValue<'_>>| value.span().start;
    let Some(table) = entry.get_ref().as_table() else {
        return Err((at(entry), String::from("each app must be a table")));
    };
    if let Some((key, _)) = table.iter().find(|(key, _)| !KEYS.contains(&key.get_ref().as_ref())) {
        return Err((
            key.span().start,
            format!("unknown key `{}`; expected one of: {}", key.get_ref(), KEYS.join(", ")),
        ));
    }
    let Some((name, _)) = string(table, "name")? else {
        return Err((at(entry), String::from("an app needs a `name`")));
    };
    let Some((category_key, category_at)) = string(table, "category")? else {
        return Err((at(entry), format!("`{name}` needs a `category`")));
    };
    let category = Category::from_key(category_key)
        .ok_or_else(|| (category_at, format!("`{name}` has an unknown category `{category_key}`")))?;
    let Some(sources) = table.get("sources") else {
        return Err((at(entry), format!("`{name}` needs `sources`")));
    };
    let Some(items) = sources.get_ref().as_array().filter(|items| !items.is_empty()) else {
        return Err((at(sources), format!("`{name}`.sources must be a list of at least one \"<source>:<package>\"")));
    };
    let mut offers = items.iter().map(read_offer).collect::<Result<Vec<_>, _>>()?;
    offers.sort_by_key(|offer| TRUST_ORDER.iter().position(|&source| source == offer.source));
    Ok(FeaturedApp { name: name.to_owned(), id: string(table, "id")?.map(|(id, _)| id.to_owned()), category, offers })
}

/// The string under `key` and where it is; `None` when the table lacks the key.
fn string<'t>(table: &'t DeTable<'_>, key: &str) -> Result<Option<(&'t str, usize)>, (usize, String)> {
    let Some(value) = table.get(key) else { return Ok(None) };
    let at = value.span().start;
    let text = value
        .get_ref()
        .as_str()
        .ok_or_else(|| (at, format!("`{key}` must be a string, found {}", value.get_ref().type_str())))?;
    Ok(Some((text, at)))
}

/// One `<source>:<package>` of `sources`.
fn read_offer(item: &Spanned<DeValue<'_>>) -> Result<Offer, (usize, String)> {
    let at = item.span().start;
    let text = item.get_ref().as_str().ok_or_else(|| (at, String::from("a source must be a string")))?;
    let (source, package) = text
        .split_once(':')
        .filter(|(_, package)| !package.is_empty())
        .ok_or_else(|| (at, format!("`{text}` must be written <source>:<package>")))?;
    let source = match source {
        "pacman" => Source::Pacman,
        "flatpak" => Source::Flatpak,
        "snap" => Source::Snap,
        "aur" => Source::Aur,
        other => return Err((at, format!("unknown source `{other}`; expected pacman, flatpak, snap or aur"))),
    };
    Ok(Offer { source, package: package.to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"
[[app]]
name = "Firefox"
id = "org.mozilla.firefox"
category = "internet"
sources = ["flatpak:org.mozilla.firefox", "pacman:firefox"]

[[app]]
name = "Visual Studio Code"
category = "development"
sources = ["aur:visual-studio-code-bin", "flatpak:com.visualstudio.code"]
"#;

    #[test]
    fn reads_the_documented_format() {
        let (apps, problems) = parse(LIST);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Firefox");
        assert_eq!(apps[0].id.as_deref(), Some("org.mozilla.firefox"));
        assert_eq!(apps[0].category, Category::Internet);
        let sources: Vec<Source> = apps[0].offers.iter().map(|offer| offer.source).collect();
        assert_eq!(sources, [Source::Pacman, Source::Flatpak], "most trusted first, whatever the file's order");
        assert_eq!(apps[1].id, None);
        assert_eq!(
            apps[1].offers[0],
            Offer { source: Source::Flatpak, package: String::from("com.visualstudio.code") }
        );
    }

    #[test]
    fn a_broken_entry_is_skipped_and_reported_where_it_is() {
        let text = r#"
[[app]]
name = "Good"
category = "game"
sources = ["pacman:good"]

[[app]]
name = "Bad category"
category = "games"
sources = ["pacman:bad"]

[[app]]
name = "Bad source"
category = "game"
sources = ["apt:bad"]

[[app]]
name = "Extra key"
category = "game"
sources = ["pacman:x"]
colour = "red"

[[app]]
category = "game"
sources = ["pacman:nameless"]

[[app]]
name = "No sources"
category = "game"
sources = []
"#;
        let (apps, problems) = parse(text);
        assert_eq!(apps.iter().map(|app| app.name.as_str()).collect::<Vec<_>>(), ["Good"]);
        let lines: Vec<usize> = problems.iter().map(|problem| problem.line).collect();
        assert_eq!(lines, [9, 15, 21, 23, 30], "{problems:?}");
        assert!(problems[0].message.contains("unknown category `games`"));
        assert!(problems[1].message.contains("unknown source `apt`"));
        assert!(problems[2].message.contains("unknown key `colour`"));
    }

    #[test]
    fn a_file_that_is_not_toml_is_one_problem() {
        let (apps, problems) = parse("[[app]\nname = ");
        assert!(apps.is_empty());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].line, 1);
    }

    #[test]
    fn an_unknown_section_is_reported() {
        let (apps, problems) = parse("[apps]\nname = \"x\"\n");
        assert!(apps.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("unknown key `apps`"));
    }
}

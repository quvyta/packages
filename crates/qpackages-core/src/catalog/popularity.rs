//! How widely packages are used: pkgstats for the repositories, Flathub's own numbers for
//! Flatpak. (The AUR's votes and popularity come with its RPC answers.) Also what Flathub
//! updated last, for the home page's "recently updated" row.
//!
//! pkgstats counts the machines that report their installed packages each month, so its top is
//! `bash` and `glibc` at 100 %; the store intersects it with the AppStream applications before
//! showing a "popular" row. Flathub counts installs over the last month.

use tinyjson::JsonValue;

use super::Problem;
use super::json;

/// The most packages pkgstats sends in one page; it refuses larger pages.
pub const PKGSTATS_MAX_LIMIT: u32 = 10_000;

/// The most applications Flathub sends in one page of a collection.
pub const FLATHUB_MAX_PER_PAGE: u32 = 250;

/// One package's share of the machines pkgstats hears from.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageShare {
    /// The package name.
    pub name: String,
    /// The share of reporting machines that have it, in percent (0 to 100).
    pub popularity: f64,
}

/// One application's standing on Flathub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlathubInstalls {
    /// The application id, the same as its AppStream id.
    pub app_id: String,
    /// How many times it was installed over the last month.
    pub installs_last_month: u64,
}

/// An application Flathub updated recently, as its collection describes it: enough for a card
/// when no catalog on disk knows the application yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlathubUpdate {
    /// The application id, the same as its AppStream id.
    pub app_id: String,
    /// Its name, in English.
    pub name: String,
    /// Its one-line summary, in English.
    pub summary: Option<String>,
    /// Flathub's main category, a freedesktop name in lower case (`audiovideo`).
    pub main_category: Option<String>,
    /// When it was last updated, as a Unix timestamp.
    pub updated_at: Option<i64>,
}

/// A page of pkgstats' package list, most used first. `limit` is capped at
/// [`PKGSTATS_MAX_LIMIT`]; the whole list is a few pages of it, with `offset` moving by `limit`.
#[must_use]
pub fn pkgstats_url(limit: u32, offset: u32) -> String {
    let limit = limit.min(PKGSTATS_MAX_LIMIT);
    format!("https://pkgstats.archlinux.de/api/packages?limit={limit}&offset={offset}")
}

/// A page of Flathub's most installed applications, counted from 1. `per_page` is capped at
/// [`FLATHUB_MAX_PER_PAGE`].
#[must_use]
pub fn flathub_popular_url(page: u32, per_page: u32) -> String {
    let per_page = per_page.min(FLATHUB_MAX_PER_PAGE);
    format!("https://flathub.org/api/v2/collection/popular?page={}&per_page={per_page}", page.max(1))
}

/// A page of Flathub's most recently updated applications, counted from 1. `per_page` is capped
/// at [`FLATHUB_MAX_PER_PAGE`].
#[must_use]
pub fn flathub_recent_url(page: u32, per_page: u32) -> String {
    let per_page = per_page.min(FLATHUB_MAX_PER_PAGE);
    format!("https://flathub.org/api/v2/collection/recently-updated?page={}&per_page={per_page}", page.max(1))
}

/// Reads a Flathub `recently-updated` answer, keeping its order (latest first). An entry without
/// an id or a name is skipped; a missing summary, category or date is left empty.
pub fn parse_flathub_recent(json: &str) -> Result<Vec<FlathubUpdate>, Problem> {
    let value = json::parse(json)?;
    let hits = list(&value, "hits")?;
    Ok(hits
        .iter()
        .filter_map(|hit| {
            let text = |key: &str| json::text(hit, key).map(str::to_owned);
            Some(FlathubUpdate {
                app_id: text("app_id")?,
                name: text("name")?,
                summary: text("summary"),
                main_category: text("main_categories"),
                updated_at: json::integer(hit, "updated_at"),
            })
        })
        .collect())
}

/// Reads a pkgstats `/api/packages` answer, keeping its order (most used first). An entry
/// without a name is skipped; one without a number counts as 0 %.
pub fn parse_pkgstats(json: &str) -> Result<Vec<PackageShare>, Problem> {
    let value = json::parse(json)?;
    let entries = list(&value, "packagePopularities")?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let name = json::text(entry, "name")?;
            let popularity = json::number(entry, "popularity").unwrap_or(0.0);
            Some(PackageShare { name: name.to_owned(), popularity })
        })
        .collect())
}

/// Reads a Flathub collection answer (`/api/v2/collection/popular`), keeping its order (most
/// installed first). An entry without an id is skipped; one without a count counts as 0.
pub fn parse_flathub_popular(json: &str) -> Result<Vec<FlathubInstalls>, Problem> {
    let value = json::parse(json)?;
    let hits = list(&value, "hits")?;
    Ok(hits
        .iter()
        .filter_map(|hit| {
            let app_id = json::text(hit, "app_id")?;
            let installs_last_month = json::count(hit, "installs_last_month").unwrap_or(0);
            Some(FlathubInstalls { app_id: app_id.to_owned(), installs_last_month })
        })
        .collect())
}

/// The list under `key`, or a problem saying it is missing.
fn list<'a>(value: &'a JsonValue, key: &str) -> Result<&'a Vec<JsonValue>, Problem> {
    json::array(value, key).ok_or_else(|| Problem { line: 1, column: 1, message: format!("no `{key}` list") })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PKGSTATS: &str = include_str!("../../tests/fixtures/catalog/pkgstats.json");
    const FLATHUB: &str = include_str!("../../tests/fixtures/catalog/flathub-popular.json");
    const RECENT: &str = include_str!("../../tests/fixtures/catalog/flathub-recently-updated.json");

    #[test]
    fn page_urls_cap_what_the_servers_refuse() {
        assert_eq!(pkgstats_url(50, 0), "https://pkgstats.archlinux.de/api/packages?limit=50&offset=0");
        assert_eq!(pkgstats_url(99_999, 10_000), "https://pkgstats.archlinux.de/api/packages?limit=10000&offset=10000");
        assert_eq!(flathub_popular_url(1, 30), "https://flathub.org/api/v2/collection/popular?page=1&per_page=30");
        assert_eq!(flathub_popular_url(0, 1000), "https://flathub.org/api/v2/collection/popular?page=1&per_page=250");
        assert_eq!(
            flathub_recent_url(1, 30),
            "https://flathub.org/api/v2/collection/recently-updated?page=1&per_page=30"
        );
    }

    #[test]
    fn reads_the_recorded_pkgstats_page() {
        let shares = parse_pkgstats(PKGSTATS).expect("a real answer");
        assert_eq!(shares.len(), 50);
        assert_eq!(shares[0], PackageShare { name: String::from("acl"), popularity: 100.0 });
        assert!(shares.windows(2).all(|pair| pair[0].popularity >= pair[1].popularity), "most used first");
    }

    #[test]
    fn reads_the_recorded_flathub_page() {
        let apps = parse_flathub_popular(FLATHUB).expect("a real answer");
        assert_eq!(apps.len(), 10);
        assert_eq!(
            apps[1],
            FlathubInstalls { app_id: String::from("org.mozilla.firefox"), installs_last_month: 186_403 }
        );
    }

    #[test]
    fn reads_the_recorded_recently_updated_page() {
        let apps = parse_flathub_recent(RECENT).expect("a real answer");
        assert_eq!(apps.len(), 14);
        assert_eq!(
            apps[1],
            FlathubUpdate {
                app_id: String::from("com.github.developer16.FFaudioConverter"),
                name: String::from("FFaudioConverter"),
                summary: Some(String::from("Batch audio converter and effects processor")),
                main_category: Some(String::from("audiovideo")),
                updated_at: Some(1_789_826_671),
            }
        );
        assert!(apps.windows(2).all(|pair| pair[0].updated_at >= pair[1].updated_at), "latest first");
        let partial = r#"{"hits":[{"app_id":"a.b.C"},{"app_id":"d.e.F","name":"F","updated_at":"soon"}]}"#;
        let apps = parse_flathub_recent(partial).expect("readable");
        assert_eq!(apps.len(), 1, "no name, no card");
        assert_eq!((apps[0].summary.as_deref(), apps[0].updated_at), (None, None));
        assert!(parse_flathub_recent("{\"hits\": 3}").is_err());
    }

    #[test]
    fn broken_answers_are_problems_not_panics() {
        assert_eq!(parse_pkgstats("{\n\"packagePopularities\": [}").map_err(|problem| problem.line), Err(2));
        assert!(parse_pkgstats("{}").is_err());
        assert!(parse_flathub_popular("[]").is_err());
        let partial = r#"{"hits":[{"name":"no id"},{"app_id":"a.b.C","installs_last_month":"lots"}]}"#;
        assert_eq!(
            parse_flathub_popular(partial),
            Ok(vec![FlathubInstalls { app_id: String::from("a.b.C"), installs_last_month: 0 }])
        );
    }
}

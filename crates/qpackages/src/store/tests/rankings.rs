//! The network's rankings kept between runs: shown at start, asked again once a day.

use std::fs::File;
use std::time::SystemTime;

use super::*;
use crate::store::cache::{self, Kept, MAX_AGE};

/// A folder of its own under the system's temporary place, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("qpackages-rankings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Keeps every ranking in `folder`, as the network sent them.
fn keep_all(folder: &Path, store: &Store) {
    cache::write(folder, Kept::Pkgstats, &fixture("pkgstats.json"));
    cache::write(folder, Kept::FlathubPopular, &core_fixture("flathub-popular.json"));
    cache::write(folder, Kept::FlathubRecent, &core_fixture("flathub-recently-updated.json"));
    assert_eq!(aur::info_urls(&aur_row_names(store)).len(), 1, "the row fits one request");
    cache::write(folder, Kept::AurRow, &fixture("aur-info-row.json"));
}

fn age(folder: &Path, kept: Kept, by: Duration) {
    let file = File::options().write(true).open(folder.join(kept.file())).expect("the kept file");
    file.set_modified(SystemTime::now() - by).expect("the date is set");
}

fn curl_calls(recorded: &Recorded) -> Vec<String> {
    recorded.command_lines().into_iter().filter(|line| line.starts_with("curl")).collect()
}

fn cached_page(recorded: &Arc<Recorded>, folder: &Path) -> Page {
    let mut page = page(recorded, true);
    page.store = std::mem::replace(&mut page.store, Store::new(machine(recorded, true), crate::sources::ALL.to_vec()))
        .with_cache(Some(folder.to_path_buf()));
    page
}

#[test]
fn recent_rankings_are_shown_from_the_cache_and_not_asked_again() {
    let scratch = Scratch::new("recent");
    // Nothing answers on the network: whatever the page shows came from the cache.
    let recorded = Arc::new(Recorded::default());
    let page = cached_page(&recorded, &scratch.0);
    keep_all(&scratch.0, &page.store);
    let h = harness(page, 96, 30, GlyphMode::Unicode);
    let ranked: Vec<&str> = h.app().store.popular.iter().take(3).map(|card| card.key.as_str()).collect();
    assert_eq!(ranked, ["org.mozilla.firefox", "org.videolan.vlc", "libreoffice-writer"], "pkgstats' order");
    assert!(h.screen().contains("AUR · 2.2k votes"), "the AUR's votes too:\n{}", h.screen());
    assert!(h.screen().contains("Halftone"), "and Flathub's latest:\n{}", h.screen());
    assert_eq!(curl_calls(&recorded), Vec::<String>::new(), "nothing is asked within a day");
}

#[test]
fn an_old_ranking_is_shown_then_asked_for_and_kept_again() {
    let scratch = Scratch::new("old");
    let recorded = online();
    let page = cached_page(&recorded, &scratch.0);
    keep_all(&scratch.0, &page.store);
    age(&scratch.0, Kept::Pkgstats, MAX_AGE * 2);
    let h = harness(page, 96, 30, GlyphMode::Unicode);
    let calls = curl_calls(&recorded);
    assert_eq!(calls.len(), 1, "only the old ranking is asked for: {calls:?}");
    assert!(calls[0].contains("pkgstats.archlinux.de"), "{calls:?}");
    let kept = cache::read(&scratch.0, Kept::Pkgstats, SystemTime::now()).expect("kept");
    assert!(kept.fresh, "the new answer is kept with a new date");
    assert_eq!(h.app().store.popular[0].key, "org.mozilla.firefox");
}

#[test]
fn a_broken_or_missing_cache_asks_the_network_and_keeps_its_answers() {
    let scratch = Scratch::new("broken");
    std::fs::create_dir_all(&scratch.0).expect("the folder");
    std::fs::write(scratch.0.join(Kept::Pkgstats.file()), "{\"packagePopularities\": [").expect("a cut file");
    let recorded = online();
    let _ = harness(cached_page(&recorded, &scratch.0), 96, 30, GlyphMode::Unicode);
    assert_eq!(curl_calls(&recorded).len(), 4, "pkgstats, Flathub twice, the AUR row: {:?}", curl_calls(&recorded));
    for kept in [Kept::Pkgstats, Kept::FlathubPopular, Kept::FlathubRecent, Kept::AurRow] {
        assert!(cache::read(&scratch.0, kept, SystemTime::now()).is_some_and(|body| body.fresh), "{kept:?} kept");
    }
}

#[test]
fn an_answer_that_did_not_come_leaves_nothing_behind() {
    let scratch = Scratch::new("offline");
    let recorded = Arc::new(Recorded::default());
    fail_url(&recorded, &popularity::pkgstats_url(5_000, 0));
    let h = harness(cached_page(&recorded, &scratch.0), 96, 30, GlyphMode::Unicode);
    assert!(!scratch.0.join(Kept::Pkgstats.file()).exists());
    assert!(h.screen().contains("Firefox"), "the starter list stands:\n{}", h.screen());
}

#[test]
fn a_kept_ranking_takes_its_place_without_moving_the_rows() {
    let scratch = Scratch::new("still");
    let recorded = Arc::new(Recorded::default());
    let mut page = cached_page(&recorded, &scratch.0);
    keep_all(&scratch.0, &page.store);
    page.held = true;
    let mut h = harness(page, 96, 30, GlyphMode::Unicode);
    h.send(Msg::Loaded(Arc::new(data::load(&machine(&recorded, true)))));
    let before = (h.find("Popular apps"), h.find("Popular in the AUR"));
    h.send(Msg::Cached(Arc::new(data::read_cached(&scratch.0, SystemTime::now()))));
    assert_eq!((h.find("Popular apps"), h.find("Popular in the AUR")), before, "{}", h.screen());
    assert_eq!(h.app().store.popular[1].key, "org.videolan.vlc", "the ranking arrived");
}

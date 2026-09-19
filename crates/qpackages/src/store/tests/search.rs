//! Searching every source at once: answers arriving at their own pace, failing, or finding
//! nothing.

use super::*;

fn keys(h: &Harness<Page>) -> Vec<String> {
    h.app()
        .store
        .search
        .as_ref()
        .map(|search| search.ranked.iter().map(|card| card.key.clone()).collect())
        .unwrap_or_default()
}

/// A page whose background work the test runs by hand, with the catalogs read.
fn held(recorded: &Arc<Recorded>) -> Harness<Page> {
    let mut page = page(recorded, true);
    page.held = true;
    let mut h = harness(page, 100, 30, GlyphMode::Unicode);
    h.send(Msg::Loaded(Arc::new(data::load(&machine(recorded, true)))));
    h
}

/// Types `query` and lets the pause pass, without running the searches.
fn start(h: &mut Harness<Page>, query: &str) -> Generation {
    h.send(Msg::Query(query.to_owned()));
    let typing = h.app().store.typing;
    h.send(Msg::Debounced(typing));
    h.app().store.search.as_ref().expect("a search started").generation
}

#[test]
fn every_source_is_searched_and_one_app_is_one_card() {
    let mut h = started(110, 34);
    search_for(&mut h, "obs");
    let screen = h.screen();
    assert!(screen.contains("results for “obs”"), "{screen}");
    assert!(screen.contains("OBS Studio"), "{screen}");
    assert!(screen.contains("Repo · Flatpak · AUR"), "the AUR build joined the repositories' card:\n{screen}");
    assert!(screen.contains("obs-vkcapture"), "{screen}");
    let obs_cards = h
        .app()
        .store
        .results
        .iter()
        .filter(|card| {
            card.app.offered_by(Source::Pacman) && card.app.offers.iter().any(|offer| offer.package == "obs-studio")
        })
        .count();
    assert_eq!(obs_cards, 1);
    assert!(!screen.contains("Searching"), "every source answered:\n{screen}");
}

#[test]
fn the_kinds_count_the_results_and_filter_them() {
    let mut h = started(110, 34);
    search_for(&mut h, "obs");
    let total = h.app().store.search.as_ref().map_or(0, |search| search.ranked.len());
    let screen = h.screen();
    assert!(
        screen
            .lines()
            .any(|line| line.starts_with('▌') && line.contains(" All ") && line.contains(&format!(" {total} "))),
        "the count beside All:\n{screen}"
    );
    // A name too long to stand beside its count gives way to its short form, never to a cut.
    let column: Vec<String> = screen.lines().map(|line| line.chars().take(18).collect()).collect();
    assert!(column.iter().all(|cell| !cell.contains('…')), "no kind is cut:\n{screen}");
    let media = column.iter().find(|cell| cell.contains("Media")).expect("the short name");
    assert!(media.trim_end().ends_with(" 1"), "with its count:\n{screen}");
    assert!(column.iter().any(|cell| cell.contains("Development")), "a name that fits stays whole:\n{screen}");
    h.set_locale("tr");
    let turkish = h.screen();
    let column: Vec<String> = turkish.lines().map(|line| line.chars().take(18).collect()).collect();
    assert!(column.iter().all(|cell| !cell.contains('…')), "no kind is cut:\n{turkish}");
    assert!(column.iter().any(|cell| cell.contains("Medya")), "{turkish}");
    h.set_locale("en");
    h.click_text("Media");
    let shown: Vec<_> = h.app().store.results.iter().map(|card| card.app.category).collect();
    assert!(!shown.is_empty());
    assert!(shown.iter().all(|&category| category == qpackages_core::catalog::category::Category::AudioVideo));
    assert!(h.screen().contains("Other"), "libraries found get their row:\n{}", h.screen());
}

#[test]
fn sources_answering_at_different_times_never_reshuffle_what_is_shown() {
    let recorded = online();
    answer_obs(&recorded);
    let mut h = held(&recorded);
    let generation = start(&mut h, "obs");
    // The catalogs answer at once; pacman and the AUR are still out.
    let first = keys(&h);
    assert!(first.contains(&String::from("com.obsproject.studio")), "{first:?}");
    h.send(Msg::Searching(generation));
    assert!(h.screen().contains("Searching: Repo, AUR"), "{}", h.screen());
    let aur = data::search_aur(recorded.as_ref(), "obs").map(aur_found);
    h.send(Msg::Found { generation, source: Source::Aur, answer: aur });
    let second = keys(&h);
    let kept: Vec<&String> = second.iter().filter(|key| first.contains(key)).collect();
    assert_eq!(kept, first.iter().collect::<Vec<_>>(), "the cards already shown keep their order");
    assert!(second.len() > first.len(), "the AUR added cards");
    assert!(h.screen().contains("Searching: Repo"), "{}", h.screen());
    assert!(!h.screen().contains("Searching: Repo, AUR"), "{}", h.screen());
    let repo = data::search_repo(recorded.as_ref(), "obs").map(|found| Found::Repo(Arc::from(found)));
    h.send(Msg::Found { generation, source: Source::Pacman, answer: repo });
    assert!(!h.screen().contains("Searching"), "{}", h.screen());
    assert!(keys(&h).contains(&String::from("pacman:obs-studio-plugin-browser")), "{:?}", keys(&h));
}

#[test]
fn an_answer_to_an_older_search_is_dropped() {
    let recorded = online();
    answer_obs(&recorded);
    let mut h = held(&recorded);
    let old = start(&mut h, "obs");
    let _ = start(&mut h, "gimp");
    let aur = data::search_aur(recorded.as_ref(), "obs").map(aur_found);
    h.send(Msg::Found { generation: old, source: Source::Aur, answer: aur });
    assert!(!keys(&h).iter().any(|key| key.starts_with("aur:")), "{:?}", keys(&h));
    assert_eq!(h.app().store.search.as_ref().map(|search| search.pending.len()), Some(2), "gimp still waits for both");
}

#[test]
fn the_spinner_waits_before_it_shows() {
    let recorded = online();
    let mut h = held(&recorded);
    let generation = start(&mut h, "obs");
    assert!(!h.screen().contains("Searching"), "not before the wait:\n{}", h.screen());
    h.send(Msg::Searching(generation));
    assert!(h.screen().contains("Searching"), "{}", h.screen());
}

#[test]
fn a_failing_source_says_so_and_the_others_stand() {
    let recorded = online();
    recorded.answer("pacman", &["-Ss", "--", "obs"], &core_fixture("pacman-ss-obs.txt"), 0);
    let url = aur::search_url("obs", aur::SearchBy::NameDesc).expect("long enough");
    fail_url(&recorded, &url);
    let mut h = harness(page(&recorded, true), 110, 34, GlyphMode::Unicode);
    search_for(&mut h, "obs");
    let screen = h.screen();
    assert!(screen.contains("AUR could not be reached."), "{screen}");
    assert!(screen.contains("Try again"), "{screen}");
    assert!(screen.contains("OBS Studio"), "the other sources' results stand:\n{screen}");
    let (x, y) = h.find("▲ AUR could not").expect("the warning line");
    let warning = h.env().theme().color("warning");
    assert_eq!(h.fg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0)), warning);
    answer_url(&recorded, &url, &fixture("aur-search-obs.json"));
    h.click_text("Try again");
    h.render();
    let screen = h.screen();
    assert!(!screen.contains("could not be reached"), "{screen}");
    assert!(screen.contains("obs-vkcapture"), "{screen}");
}

#[test]
fn too_many_aur_results_ask_for_more_letters_not_a_retry() {
    let recorded = online();
    recorded.answer("pacman", &["-Ss", "--", "py"], "", 1);
    let url = aur::search_url("py", aur::SearchBy::NameDesc).expect("long enough");
    answer_url(&recorded, &url, &core_fixture("aur-error-too-many.json"));
    let mut h = harness(page(&recorded, true), 110, 34, GlyphMode::Unicode);
    search_for(&mut h, "py");
    let screen = h.screen();
    assert!(screen.contains("AUR found too many packages. Type more to narrow it down."), "{screen}");
    assert!(!screen.contains("Try again"), "{screen}");
}

#[test]
fn nothing_found_is_said_once_every_source_has_answered() {
    let recorded = online();
    recorded.answer("pacman", &["-Ss", "--", "zzqx"], "", 1);
    let url = aur::search_url("zzqx", aur::SearchBy::NameDesc).expect("long enough");
    answer_url(&recorded, &url, &core_fixture("aur-search-empty.json"));
    let mut h = harness(page(&recorded, true), 110, 34, GlyphMode::Unicode);
    search_for(&mut h, "zzqx");
    let screen = h.screen();
    assert!(screen.contains("Nothing found for “zzqx”"), "{screen}");
    assert!(screen.contains("Check the spelling or try another word."), "{screen}");
}

#[test]
fn nothing_found_is_not_said_while_sources_are_still_searching() {
    let recorded = online();
    let mut h = held(&recorded);
    let _ = start(&mut h, "zzqx");
    assert!(!h.screen().contains("Nothing found"), "{}", h.screen());
}

#[test]
fn sorting_by_name_reorders_at_once() {
    let mut h = started(110, 34);
    search_for(&mut h, "obs");
    h.send(Msg::Sort(1));
    let names: Vec<String> = h.app().store.results.iter().map(|card| card.app.name.default.to_lowercase()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn clearing_the_field_goes_home_and_drops_what_is_on_its_way() {
    let recorded = online();
    answer_obs(&recorded);
    let mut h = held(&recorded);
    let generation = start(&mut h, "obs");
    h.send(Msg::Query(String::new()));
    assert!(h.app().store.search.is_none());
    assert!(h.screen().contains("Popular apps"), "{}", h.screen());
    let aur = data::search_aur(recorded.as_ref(), "obs").map(aur_found);
    h.send(Msg::Found { generation, source: Source::Aur, answer: aur });
    assert!(h.app().store.search.is_none(), "a late answer does not bring the search back");
}

#[test]
fn a_result_opens_its_page_and_the_aur_says_who_keeps_it() {
    let recorded = online();
    answer_obs(&recorded);
    let url = aur::info_urls(&["obs-vkcapture"]).remove(0);
    answer_url(&recorded, &url, &fixture("aur-info-row.json").replace("visual-studio-code-bin", "obs-vkcapture"));
    let mut h = harness(page(&recorded, true), 110, 34, GlyphMode::Unicode);
    search_for(&mut h, "obs");
    h.click_text("obs-vkcapture");
    h.render();
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    for text in ["Votes  1708", "Maintainer  maintainer-1", "Last updated  2025-09-04", "License  custom: commercial"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn offline_a_catalog_application_leads_the_packages_that_match_as_well() {
    let recorded = Arc::new(Recorded::default());
    answer_obs(&recorded);
    let mut h = harness(page(&recorded, true), 110, 34, GlyphMode::Unicode);
    search_for(&mut h, "obs");
    let first = h.app().store.results.first().map(|card| card.key.clone());
    assert_eq!(first.as_deref(), Some("com.obsproject.studio"), "no popularity arrived, the catalog decides");
}
#[test]
fn one_info_request_after_a_search_gives_cards_and_pages_the_same_record() {
    let recorded = online();
    answer_obs(&recorded);
    let found = data::search_aur(recorded.as_ref(), "obs").expect("the recorded search");
    let first = data::first_results(&found, "obs");
    assert_eq!(&first[..3], ["obs-pipewire-audio-capture", "obs-studio-liberty", "obs-wayland-hotkeys-git"]);
    let urls = aur::info_urls(&first);
    assert_eq!(urls.len(), 1, "one request");
    answer_url(&recorded, &urls[0], &fixture("aur-info-obs.json"));
    let mut h = harness(page(&recorded, true), 110, 34, GlyphMode::Unicode);
    search_for(&mut h, "obs");
    let info_calls = |recorded: &Recorded| {
        recorded.command_lines().into_iter().filter(|line| line.contains("/rpc/v5/info?")).count()
    };
    let before = info_calls(&recorded);
    let search = h.app().store.search.as_ref().expect("a search");
    assert_eq!(search.aur_detailed.len(), 20, "every result was detailed");
    let capture = search.aur.iter().find(|package| package.name == "obs-vkcapture").expect("found");
    assert_eq!(capture.licenses, ["GPL-2.0-or-later"], "the full record took the summary's place");
    h.click_text("obs-vkcapture");
    h.render();
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    for text in ["License  GPL-2.0-or-later", "Votes  25", "Maintainer  maintainer-6", "Depends on"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert_eq!(info_calls(&recorded), before, "the page asked the AUR nothing more");
}

#[test]
fn more_than_fifty_results_ask_about_the_first_fifty_only() {
    let packages: Vec<AurPackage> = (0..80)
        .map(|index| AurPackage {
            name: format!("obs-plugin-{index:02}"),
            popularity: f64::from(index),
            ..AurPackage::default()
        })
        .collect();
    let first = data::first_results(&packages, "obs");
    assert_eq!(first.len(), data::AUR_DETAILED);
    assert_eq!(first[0], "obs-plugin-79", "the most popular first among equal matches");
    assert_eq!(aur::info_urls(&first).len(), 1);
}

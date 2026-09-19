//! The store driven through the screen, on a pretend machine: catalogs from fixture folders, and
//! pacman and curl answering from recordings. Nothing here reaches the network or pacman.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qpackages_core::catalog::{aur, net, popularity};
use qpackages_core::sources::{AurHelper, Availability};

use super::*;
use crate::runner::Recorded;

mod mount;
mod screen;
mod search;

/// The store as an application, for the harness. It keeps every request the store sends out,
/// and with `held` it drops the store's background work, so a test decides when each source
/// answers.
struct Page {
    store: Store,
    requests: Vec<Request>,
    held: bool,
    init: bool,
}

impl App for Page {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        if let Msg::Request(request) = &msg {
            self.requests.push(request.clone());
        }
        let command = self.store.update(msg);
        if self.held { Command::none() } else { command }
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        self.store.view(ui);
    }

    fn init(&mut self) -> Command<Msg> {
        if self.init && !self.held { self.store.init() } else { Command::none() }
    }

    // As the application does: the page's keys go to the page while it is shown.
    fn action(&self, name: &str) -> Option<Msg> {
        self.store.action(name)
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(fixtures().join(name)).expect("the fixture is readable")
}

fn core_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/catalog").join(name);
    std::fs::read_to_string(path).expect("the fixture is readable")
}

/// Every source but Snap is on this pretend machine.
fn sources() -> Sources {
    let ready = |program: &str| Availability::Ready { program: Path::new("/usr/bin").join(program) };
    Sources {
        pacman: ready("pacman"),
        aur: ready("paru"),
        aur_helper: Some(AurHelper::Paru),
        flatpak: ready("flatpak"),
        snap: Availability::Missing,
        fakeroot: true,
    }
}

/// A machine with the repositories' catalog when `catalog`, and Flathub's always.
fn machine(recorded: &Arc<Recorded>, catalog: bool) -> Machine {
    let swcatalog = if catalog { fixtures().join("swcatalog") } else { fixtures().join("no-such-folder") };
    Machine { runner: Arc::clone(recorded) as Arc<dyn Runner>, swcatalog, flatpak: vec![fixtures().join("flatpak")] }
}

use crate::runner::Runner;

/// Answers `url` through curl with `body`.
fn answer_url(recorded: &Recorded, url: &str, body: &str) {
    recorded.answer(net::CURL, &net::curl_args(url), body, 0);
}

/// Fails `url` the way curl fails when the host cannot be reached.
fn fail_url(recorded: &Recorded, url: &str) {
    recorded.fail(net::CURL, &net::curl_args(url), "curl: (6) Could not resolve host", 6);
}

/// The names of the AUR row's packages, which the store asks the AUR about when it starts.
fn aur_row_names(store: &Store) -> Vec<String> {
    store
        .aur_all
        .iter()
        .flat_map(|card| card.app.offers.iter().filter(|offer| offer.source == Source::Aur))
        .map(|offer| offer.package.clone())
        .collect()
}

/// A runner where the network answers: pkgstats, Flathub and the AUR about its row.
fn online() -> Arc<Recorded> {
    let recorded = Arc::new(Recorded::default());
    answer_url(&recorded, &popularity::pkgstats_url(5_000, 0), &fixture("pkgstats.json"));
    answer_url(&recorded, &popularity::flathub_popular_url(1, 250), &core_fixture("flathub-popular.json"));
    let store = Store::new(machine(&recorded, true), crate::sources::ALL.to_vec());
    for url in aur::info_urls(&aur_row_names(&store)) {
        answer_url(&recorded, &url, &fixture("aur-info-row.json"));
    }
    recorded
}

/// Adds the answers to a search for "obs": pacman's and the AUR's.
fn answer_obs(recorded: &Recorded) {
    recorded.answer("pacman", &["-Ss", "--", "obs"], &core_fixture("pacman-ss-obs.txt"), 0);
    let url = aur::search_url("obs", aur::SearchBy::NameDesc).expect("long enough");
    answer_url(recorded, &url, &fixture("aur-search-obs.json"));
}

fn page(recorded: &Arc<Recorded>, catalog: bool) -> Page {
    let mut store = Store::new(machine(recorded, catalog), crate::sources::ALL.to_vec());
    store.machine_read(&sources(), ["firefox".to_owned(), "bash".to_owned()]);
    Page { store, requests: Vec::new(), held: false, init: true }
}

/// A harness on `page`, `width` × `height`, in English and `mode`, with the start-up work done.
fn harness(page: Page, width: u16, height: u16, mode: GlyphMode) -> Harness<Page> {
    let mut h = Harness::with_env(page, crate::test_env(), width, height);
    h.set_locale("en").set_glyph_mode(mode);
    // The catalogs and the network answer in the background; one more step delivers them.
    h.render();
    h
}

/// The store on a machine with everything, after start-up.
fn started(width: u16, height: u16) -> Harness<Page> {
    let recorded = online();
    answer_obs(&recorded);
    harness(page(&recorded, true), width, height, GlyphMode::Unicode)
}

/// Types `text` into the search and waits out the pause, so the search runs.
fn search_for(h: &mut Harness<Page>, text: &str) {
    h.press("/");
    h.type_text(text);
    h.advance(DEBOUNCE);
    h.render();
}

#[test]
fn the_starter_list_reads_cleanly_and_every_entry_can_be_installed_from_somewhere() {
    let (apps, problems) = featured::parse(FEATURED);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(apps.len() >= 60, "{}", apps.len());
    assert!(apps.iter().all(|app| app.summary.as_ref().is_some_and(|summary| summary.turkish.is_some())));
    let text = FEATURED.to_lowercase();
    assert!(!text.split(|c: char| !c.is_alphanumeric()).any(|word| word == "ai"), "the publish scan refuses the word");
}

#[test]
fn the_language_files_carry_the_store_in_both_languages() {
    let env = crate::test_env();
    assert_eq!(env.diagnostics(), &[]);
    assert_eq!(env.i18n().missing_keys("tr", "en"), Vec::<String>::new());
}

#[test]
fn only_what_the_flow_runs_today_becomes_a_transaction() {
    let offer = |source, package: &str| Offer { source, package: package.to_owned() };
    let install = Request::Install(vec![
        offer(Source::Pacman, "gimp"),
        offer(Source::Aur, "spotify"),
        offer(Source::Flatpak, "x"),
    ]);
    assert_eq!(transaction(&install), Some(crate::transaction::Action::Install(vec![String::from("gimp")])));
    assert_eq!(transaction(&Request::Install(vec![offer(Source::Aur, "spotify")])), None, "nothing it can run");
    let remove = Request::Remove(vec![offer(Source::Aur, "spotify")]);
    assert_eq!(transaction(&remove), Some(crate::transaction::Action::Remove(vec![String::from("spotify")])));
    assert_eq!(transaction(&Request::OpenSettings(None)), None);
}

#[test]
fn a_compressed_catalog_reads_like_a_plain_one() {
    let folder = std::env::temp_dir().join(format!("qpackages-store-gz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("a temporary folder");
    let gz = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/catalog/arch-extra.xml.gz");
    std::fs::copy(gz, folder.join("extra.xml.gz")).expect("the fixture copies");
    std::fs::write(folder.join("broken.xml.gz"), b"not gzip").expect("a broken file");
    let recorded = Arc::new(Recorded::default());
    let loaded = data::load(&Machine { swcatalog: folder.clone(), ..machine(&recorded, true) });
    let _ = std::fs::remove_dir_all(&folder);
    assert!(loaded.has_repo_catalog);
    assert!(loaded.repo.iter().any(|component| component.id == "org.gimp.GIMP"), "the broken file is passed over");
}

#[test]
fn the_catalogs_on_disk_are_read_and_a_missing_folder_is_no_error() {
    let recorded = Arc::new(Recorded::default());
    let loaded = data::load(&machine(&recorded, true));
    assert!(loaded.has_repo_catalog);
    assert!(loaded.repo.iter().any(|component| component.id == "com.obsproject.Studio"));
    assert!(loaded.flathub.iter().any(|component| component.id == "org.videolan.VLC"));
    assert!(loaded.flathub.iter().all(|component| component.kind.is_listed()), "runtimes are not cards");
    let obs = loaded.component("com.obsproject.Studio").expect("OBS is in the catalog");
    assert!(obs.description.is_some());
    let missing = data::load(&machine(&recorded, false));
    assert!(!missing.has_repo_catalog);
    assert!(missing.repo.is_empty());
    assert!(!missing.flathub.is_empty(), "Flathub's catalog does not depend on the repositories'");
}

#[test]
fn pacman_finding_nothing_is_an_answer_and_a_missing_pacman_is_a_failure() {
    let recorded = Recorded::default();
    recorded.answer("pacman", &["-Ss", "--", "zzzz"], "", 1);
    assert_eq!(data::search_repo(&recorded, "zzzz"), Ok(Vec::new()));
    assert_eq!(data::search_repo(&recorded, "other"), Err(Failure::Unreachable));
    assert_eq!(data::search_repo(&recorded, "  "), Ok(Vec::new()), "nothing is asked for an empty term");
    let too_many = aur::search_url("py", aur::SearchBy::NameDesc).expect("long enough");
    answer_url(&recorded, &too_many, &core_fixture("aur-error-too-many.json"));
    assert_eq!(data::search_aur(&recorded, "py"), Err(Failure::TooMany));
    assert_eq!(data::search_aur(&recorded, "x"), Ok(Vec::new()), "one letter is never sent");
}

#[test]
fn the_page_waits_for_nothing_but_start_up_and_starts_once() {
    let recorded = online();
    let mut store = Store::new(machine(&recorded, true), crate::sources::ALL.to_vec());
    let _ = store.init();
    let _ = store.init();
    assert!(store.started);
    assert!(recorded.calls().is_empty(), "init only queues work; nothing ran on this thread");
}

#[test]
fn the_debounce_waits_for_typing_to_pause_and_asks_once() {
    let recorded = online();
    answer_obs(&recorded);
    let mut h = harness(page(&recorded, true), 96, 30, GlyphMode::Unicode);
    let searches = |recorded: &Recorded| {
        recorded.command_lines().into_iter().filter(|line| line.starts_with("pacman -Ss")).collect::<Vec<_>>()
    };
    h.press("/");
    h.type_text("obs");
    assert!(h.app().store.search.is_none(), "typing has not paused yet");
    assert!(searches(&recorded).is_empty());
    h.advance(DEBOUNCE);
    h.render();
    assert!(h.app().store.search.is_some());
    assert_eq!(searches(&recorded), ["pacman -Ss -- obs"], "one search, for the whole word");
}

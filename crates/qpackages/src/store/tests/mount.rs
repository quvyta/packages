//! Discover as the frame's first tab: the app opens on it, its requests reach the transaction
//! flow and the settings page, and its keys work only while it is the open tab.

use qframe::storage::Settings;
use qpackages_core::pacman::command::print_install;

use super::*;
use crate::app::{Machine as AppMachine, Msg as AppMsg, Qpackages, Tab};
use crate::helper::session::InProcess;
use crate::testing::{Sample, Scratch, programs};

/// The frame on a made-up machine with firefox installed, Discover reading the fixture
/// catalogs and the network answering from `recorded`.
fn app(recorded: &Arc<Recorded>, scratch: &Scratch) -> Harness<Qpackages> {
    let settings = Settings::parse_str("settings.toml", "").schema(crate::settings::schema());
    let flatpak = [fixtures().join("flatpak")];
    let machine = AppMachine {
        dbpath: &scratch.local(),
        sync_dir: &scratch.sync(),
        applications: &scratch.applications(),
        check_dir: None,
        lock_dir: &scratch.lock(),
        lookup: Arc::new(programs),
        runner: Arc::clone(recorded) as Arc<dyn Runner>,
        helper: InProcess::new(recorded, 0).start_fn(),
        uid: Some(1000),
        utc_offset: 0,
        app_catalog: &fixtures().join("swcatalog"),
        flatpak_catalogs: &flatpak,
        appearance: crate::appearance_in(scratch.root()),
        snap_socket: &scratch.root().join("snapd.socket"),
        first_run: None,
    };
    let mut h = Harness::with_env(Qpackages::new(machine, &settings), crate::locales::env(), 100, 30);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.render();
    h
}

fn scratch(name: &str) -> Scratch {
    Scratch::new(name, &[Sample::new("firefox", "143.0.1-1", "Fast, Private & Safe Web Browser").app()])
}

#[test]
fn the_app_opens_on_discover_with_the_catalog_read_and_the_installed_marked() {
    let recorded = online();
    let machine = scratch("discover-opens");
    let h = app(&recorded, &machine);
    assert_eq!(h.app().tab(), Tab::Discover);
    let screen = h.screen();
    for text in ["Discover", "Installed", "Updates", "Popular apps", "LibreOffice Writ", "Repo · Installed ✓"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn the_tabs_go_by_number_and_discover_keeps_its_search() {
    let recorded = online();
    answer_obs(&recorded);
    let machine = scratch("discover-tabs");
    let mut h = app(&recorded, &machine);
    h.press("/");
    h.type_text("obs");
    h.advance(DEBOUNCE);
    h.render();
    assert!(h.screen().contains("results for “obs”"), "{}", h.screen());
    h.press("ctrl+2");
    assert_eq!(h.app().tab(), Tab::Installed);
    h.press("ctrl+3");
    assert_eq!(h.app().tab(), Tab::Updates);
    h.press("ctrl+1");
    assert_eq!(h.app().tab(), Tab::Discover);
    assert!(h.screen().contains("results for “obs”"), "the search waited:\n{}", h.screen());
}

#[test]
fn an_install_asked_in_discover_starts_the_usual_confirmed_flow() {
    let recorded = online();
    recorded.answer("pacman", &print_install(&["obs-studio"]), "", 0);
    let machine = scratch("discover-install");
    let mut h = app(&recorded, &machine);
    let obs = Offer { source: Source::Pacman, package: String::from("obs-studio") };
    h.send(AppMsg::Discover(Msg::Request(Request::Install(vec![obs]))));
    let asked = recorded.command_lines();
    assert!(
        asked.iter().any(|line| line.starts_with("pacman -S --print") && line.ends_with("obs-studio")),
        "{asked:?}"
    );
}

#[test]
fn the_app_catalog_is_installed_through_the_same_flow() {
    let recorded = online();
    recorded.answer("pacman", &print_install(&[CATALOG_PACKAGE]), "", 0);
    let machine = scratch("discover-catalog");
    let mut h = app(&recorded, &machine);
    let catalog = Offer { source: Source::Pacman, package: CATALOG_PACKAGE.to_owned() };
    h.send(AppMsg::Discover(Msg::Request(Request::Install(vec![catalog]))));
    let asked = recorded.command_lines();
    assert!(
        asked.iter().any(|line| line.starts_with("pacman -S --print") && line.ends_with(CATALOG_PACKAGE)),
        "{asked:?}"
    );
}

#[test]
fn what_the_flow_cannot_run_starts_nothing() {
    let recorded = online();
    let machine = scratch("discover-aur");
    let mut h = app(&recorded, &machine);
    let spotify = Offer { source: Source::Aur, package: String::from("spotify") };
    h.send(AppMsg::Discover(Msg::Request(Request::Install(vec![spotify]))));
    assert!(!recorded.command_lines().iter().any(|line| line.starts_with("pacman") && line.contains("spotify")));
    assert!(h.screen().contains("Popular apps"), "{}", h.screen());
}

#[test]
fn a_source_leads_to_the_settings_page() {
    let recorded = online();
    let machine = scratch("discover-settings");
    let mut h = app(&recorded, &machine);
    h.send(AppMsg::Discover(Msg::Request(Request::OpenSettings(Some(Source::Snap)))));
    assert!(h.app().settings_open(), "{}", h.screen());
}

#[test]
fn discover_keys_work_only_while_it_is_open() {
    let recorded = online();
    let machine = scratch("discover-keys");
    let mut h = app(&recorded, &machine);
    h.send(AppMsg::Discover(Msg::SeeAll(Section::Popular)));
    h.press("esc");
    assert!(h.screen().contains("Sources"), "esc came back home:\n{}", h.screen());
    h.send(AppMsg::Discover(Msg::Toggle(Grid::Row(Section::Popular), 1)));
    h.press("ctrl+2");
    h.press("ctrl+enter");
    assert!(!recorded.command_lines().iter().any(|line| line.starts_with("pacman -S --print")), "not from Installed");
}

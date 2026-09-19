use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::env::Env;
use qframe::icons::GlyphMode;
use qpackages_core::pacman::Update;

use super::*;
use crate::helper::session::InProcess;
use crate::runner::Call;
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in};
use crate::updates::check::Found;

mod flow;
mod polkit;

/// Built-in files plus the compiled-in locales and keymap, as the runtime loads them.
fn env() -> Env {
    crate::test_env()
}

/// The recorded local database: bash, plus two records that cannot be read.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/localdb")
}

/// A path nothing is at: the launchers and the repository databases of a machine that has none.
fn nowhere() -> PathBuf {
    fixture().join("nowhere")
}

/// A pretend machine with pacman, paru and fakeroot but neither flatpak nor snap.
fn machine(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// The application on the pretend machine, running programs from `recorded` as user `uid`, with
/// transactions carried out by helpers from `helper`. It has no launchers and nowhere to keep a
/// copy for the update check, so the only program it runs by itself is the question of which
/// packages no repository offers.
fn app_on(
    settings: &str,
    recorded: &Arc<Recorded>,
    helper: &Arc<InProcess>,
    uid: Option<u32>,
    lock_dir: &Path,
) -> Qpackages {
    let settings = Settings::parse_str("settings.toml", settings).schema(settings::schema());
    let machine = Machine {
        dbpath: &fixture(),
        sync_dir: &nowhere(),
        applications: &nowhere(),
        check_dir: None,
        lock_dir,
        lookup: Arc::new(machine),
        runner: Arc::clone(recorded) as Arc<dyn crate::runner::Runner>,
        helper: helper.start_fn(),
        uid,
        utc_offset: 0,
        app_catalog: &nowhere(),
        flatpak_catalogs: &[],
    };
    Qpackages::new(machine, &settings)
}

/// The calls a test's actions made, without those the application makes on its own when it
/// starts: the question of which packages no repository offers, the AUR's update check, and
/// Discover asking the network for its rankings.
fn after_reads(recorded: &Recorded) -> Vec<Call> {
    recorded
        .calls()
        .into_iter()
        .filter(|call| !(call.args == ["-Qqm"] || call.args == ["-Qua"] || call.program == "curl"))
        .collect()
}

fn app(settings: &str) -> Qpackages {
    let recorded = Arc::new(Recorded::default());
    app_on(settings, &recorded, &InProcess::new(&recorded, 0), Some(1000), &fixture())
}

/// The screen on the fixture on the Installed tab, showing every package: bash is a package,
/// not an application.
fn harness(width: u16, height: u16) -> Harness<Qpackages> {
    let mut h = Harness::with_env(app(""), env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.send(Msg::Tab(Tab::Installed.index()));
    h.send(Msg::Installed(installed::Msg::Show(1)));
    h
}

/// Every tab's message, in the order of the header.
fn every_tab() -> Vec<Msg> {
    (0..TABS.len()).map(Msg::Tab).collect()
}

#[test]
fn qpac_opens_on_discover_with_the_starter_list() {
    let mut h = Harness::with_env(app(""), env(), 100, 30);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    assert_eq!(h.app().tab, Tab::Discover);
    let screen = h.screen();
    let header = screen.lines().next().unwrap_or_default();
    let (discover, installed) = (header.find("Discover"), header.find("Installed"));
    assert!(discover.is_some() && discover < installed, "Discover is the first tab:\n{screen}");
    for text in ["Search apps and packages", "Popular apps", "Firefox", "Popular in the AUR"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.press("/");
    assert!(h.is_focused("store-search"), "the search key reaches Discover's search");
}

fn update(name: &str, from: &str, to: &str) -> Update {
    Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
}

/// What a check that found three repository updates reports, at 14:02 UTC.
fn three_updates() -> Found {
    let repo = vec![
        update("linux", "6.18.1-1", "6.18.2-1"),
        update("mesa", "25.2.3-1", "25.2.4-1"),
        update("bash", "5.3.15-1", "5.3.16-1"),
    ];
    Found { at: 1_789_999_320, repo: Ok(repo), aur: None }
}

#[test]
fn the_language_files_load_without_problems_and_turkish_is_complete() {
    let env = env();
    assert_eq!(env.diagnostics(), &[]);
    assert_eq!(env.i18n().missing_keys("tr", "en"), Vec::<String>::new());
}

/// The application before its first read: `init` is not run, so the screen shows what a user
/// sees while the database is being opened.
struct Unread(Qpackages);

impl App for Unread {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        self.0.update(msg)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        self.0.view(ui);
    }

    fn resized(&self, size: Size) -> Option<Msg> {
        self.0.resized(size)
    }
}

#[test]
fn the_frame_stands_before_the_first_read_and_fills_when_it_answers() {
    let mut h = Harness::with_env(Unread(app("")), env(), 100, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    assert!(h.screen().contains("Popular apps"), "Discover stands on its starter list:\n{}", h.screen());
    h.send(Msg::Tab(Tab::Installed.index()));
    let screen = h.screen();
    for text in ["qpac", "Installed", "Updates", "Reading the installed packages"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("No packages are installed"), "an empty table is not claimed:\n{screen}");
    assert!(!screen.contains("packages ·") && !screen.contains("0 apps"), "no count before the read:\n{screen}");
    h.send(Msg::Installed(installed::Msg::Show(1)));
    let snapshot = h.app().0.reload.read();
    h.send(Msg::Reloaded(snapshot));
    let screen = h.screen();
    assert!(screen.contains("bash"), "the packages arrived:\n{screen}");
    assert!(!screen.contains("Reading"), "{screen}");
    let mut unread = Harness::with_env(Unread(app("")), env(), 100, 24);
    unread.set_locale("tr");
    unread.send(Msg::Tab(Tab::Installed.index()));
    let screen = unread.screen();
    for text in ["Kurulu paketler okunuyor", "Kurulu", "Güncellemeler"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn the_first_read_starts_from_init_and_the_first_frame_already_shows_it() {
    let h = harness(100, 24);
    assert!(h.app().library.is_some(), "the read answered before the first frame was painted");
    assert!(h.screen().contains("bash"), "{}", h.screen());
}

#[test]
fn the_header_has_the_name_the_tabs_and_the_settings_button_and_no_sidebar() {
    let h = harness(100, 24);
    let screen = h.screen();
    let header = screen.lines().next().expect("a header line");
    for text in ["qpac", "Discover", "Installed", "Updates", "▤"] {
        assert!(header.contains(text), "`{text}` is not in the header:\n{screen}");
    }
    assert!(header.trim_end().ends_with('▤'), "the settings button stands at the right end:\n{screen}");
    for gone in ["Pacman", "Flatpak", "Snap"] {
        assert!(!screen.contains(gone), "the sources sidebar is gone, `{gone}` with it:\n{screen}");
    }
    let mut h = h;
    let (x, y) = h.find("qpac").expect("the name is drawn");
    let name = h.fg(u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
    let (x, y) = h.find("Updates").expect("a tab");
    assert_ne!(name, h.fg(u16::try_from(x).unwrap(), u16::try_from(y).unwrap()), "the name is in the accent");
    h.set_glyph_mode(GlyphMode::Ascii);
    assert!(h.screen().lines().next().is_some_and(|line| line.trim_end().ends_with('*')), "{}", h.screen());
    h.set_glyph_mode(GlyphMode::Nerd);
    assert!(h.screen().lines().next().is_some_and(|line| line.contains('\u{f013}')), "{}", h.screen());
}

#[test]
fn tabs_open_by_click_by_number_and_by_arrow_and_keep_their_state() {
    let mut h = harness(100, 24);
    h.send(Msg::Installed(installed::Msg::Search("ba".to_owned())));
    h.click_text("Updates");
    assert_eq!(h.app().tab, Tab::Updates);
    assert!(!h.screen().contains("5.3.15-1"), "the Installed page is gone:\n{}", h.screen());
    h.press("ctrl+2");
    assert_eq!(h.app().tab, Tab::Installed);
    let screen = h.screen();
    assert!(screen.contains("ba") && screen.contains("5.3.15-1"), "the search and its rows stay:\n{screen}");
    h.press("alt+right");
    assert_eq!(h.app().tab, Tab::Updates);
    h.press("alt+right");
    assert_eq!(h.app().tab, Tab::Updates, "the last tab stays the last");
    h.press("alt+left");
    assert_eq!(h.app().tab, Tab::Installed);
    h.press("alt+left");
    assert_eq!(h.app().tab, Tab::Discover);
    h.press("alt+left");
    assert_eq!(h.app().tab, Tab::Discover, "the first tab stays the first");
    h.press("ctrl+3");
    assert_eq!(h.app().tab, Tab::Updates);
    h.press("ctrl+1");
    assert_eq!(h.app().tab, Tab::Discover);
    assert!(h.app().action("tab-9").is_none() && h.app().action("tab-0").is_none());
}

#[test]
fn the_search_key_reaches_the_installed_search_only_where_it_is() {
    let mut h = harness(100, 24);
    h.press("/");
    assert!(h.is_focused("search"), "the search key puts the keyboard in the field");
    h.type_text("zinga");
    assert!(h.screen().contains("No package matches the search"), "{}", h.screen());
    h.press("ctrl+3");
    h.press("/");
    assert!(!h.is_focused("search"), "the Updates tab has no search");
}

#[test]
fn the_settings_button_and_its_key_open_the_page_and_escape_and_back_close_it() {
    let mut h = harness(100, 24);
    let header = h.screen().lines().next().expect("a header").to_owned();
    let x = header.chars().position(|c| c == '▤').expect("the settings button");
    h.click(i32::try_from(x).unwrap(), 0);
    assert!(h.app().settings_open);
    let screen = h.screen();
    for text in ["Settings", "Back", "Sources", "Permission", "Appearance"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.press("esc");
    assert!(!h.app().settings_open, "escape goes back");
    h.press("ctrl+,");
    assert!(h.app().settings_open, "ctrl+, opens it");
    h.click_text("Back");
    assert!(!h.app().settings_open);
    assert!(h.screen().contains("bash"), "the tab is back as it was:\n{}", h.screen());
    h.press("ctrl+,");
    h.click_text("Updates");
    assert!(!h.app().settings_open, "a tab closes the page");
    assert_eq!(h.app().tab, Tab::Updates);
}

#[test]
fn the_settings_button_brightens_under_the_pointer() {
    let mut h = harness(100, 24);
    let header = h.screen().lines().next().expect("a header").to_owned();
    let x = u16::try_from(header.chars().position(|c| c == '▤').expect("the settings button")).unwrap();
    let rest = h.bg(x, 0);
    h.hover(i32::from(x), 0);
    assert_ne!(h.bg(x, 0), rest, "hover raises the button");
}

#[test]
fn the_updates_tab_carries_the_number_of_waiting_updates() {
    let mut h = harness(120, 24);
    assert!(!h.screen().lines().next().unwrap_or_default().contains('●'), "no updates, no badge");
    h.send(Msg::Updates(updates::Msg::Checked(three_updates())));
    let header = h.screen().lines().next().unwrap_or_default().to_owned();
    let updates = header.find("Updates").expect("the tab");
    let badge = header.find('●').expect("the badge");
    assert!(
        badge > updates && header[badge..].split_whitespace().nth(1) == Some("3"),
        "the count follows the tab:\n{header}"
    );
    let before_badge = header[updates + "Updates".len()..badge].trim();
    assert!(before_badge.is_empty(), "nothing stands between the tab and its count:\n{header}");
}

#[test]
fn running_as_root_is_said_in_the_header_and_on_the_aur_row() {
    let recorded = Arc::new(Recorded::default());
    let app = app_on("", &recorded, &InProcess::new(&recorded, 0), Some(0), &fixture());
    let mut h = Harness::with_env(app, env(), 120, 30);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    assert!(h.screen().contains("Running as root"), "{}", h.screen());
    h.press("ctrl+,");
    assert!(h.screen().contains("not as root"), "{}", h.screen());
    let mut ordinary = harness(120, 30);
    assert!(!ordinary.screen().contains("Running as root"));
    ordinary.press("ctrl+,");
    assert!(!ordinary.screen().contains("not as root"), "{}", ordinary.screen());
}

#[test]
fn turkish_uses_its_own_words() {
    let mut h = harness(100, 24);
    h.set_locale("tr");
    h.send(Msg::Installed(installed::Msg::Select(0)));
    let screen = h.screen();
    for text in ["İstenen", "Kurulma nedeni", "Kurulum tarihi", "Bağımlılıklar", "Kurulu", "Bütün paketler", "ara"]
    {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains('⟦'), "a key is missing in Turkish:\n{screen}");
    for page in every_tab().into_iter().chain([Msg::OpenSettings]) {
        h.send(page);
        assert!(!h.screen().contains('⟦'), "a key is missing in Turkish:\n{}", h.screen());
    }
}

#[test]
fn every_page_keeps_the_rules_in_ascii_and_on_narrow_screens() {
    for (width, height) in [(36, 16), (60, 20), (80, 24), (120, 30)] {
        let mut h = harness(width, height);
        h.set_glyph_mode(GlyphMode::Ascii);
        h.send(Msg::Updates(updates::Msg::Checked(three_updates())));
        h.send(Msg::Installed(installed::Msg::Select(0)));
        for page in every_tab().into_iter().chain([Msg::OpenSettings]) {
            h.send(page);
            let screen = h.screen();
            assert!(screen.contains("qpac"), "{width}x{height}:\n{screen}");
            for forbidden in ['[', ']', '{', '}', '|'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
            }
            assert!(!screen.contains("-->") && !screen.contains("==="), "{width}x{height}:\n{screen}");
        }
    }
}

/// With `QUVYTA_REVIEW=1`, writes every page in both languages, wide and narrow, to
/// `target/qpackages-review.html` in colour for a visual review.
#[test]
fn visual_review() {
    if std::env::var_os("QUVYTA_REVIEW").is_none() {
        return;
    }
    let mut fragments = Vec::new();
    for (width, height) in [(120, 30), (80, 24), (60, 20), (36, 16)] {
        for locale in ["en", "tr"] {
            let mut h = harness(width, height);
            h.set_locale(locale);
            h.send(Msg::Updates(updates::Msg::Checked(three_updates())));
            h.send(Msg::Installed(installed::Msg::Select(0)));
            let pages = TABS.iter().map(|tab| (tab.key(), Msg::Tab(tab.index())));
            for (name, page) in pages.chain([("settings", Msg::OpenSettings)]) {
                h.send(page);
                fragments.push(h.html(&format!("{name} {locale} {width}x{height}")));
                println!("{name} {locale} {width}x{height}\n{}", h.screen());
            }
        }
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/qpackages-review.html");
    std::fs::write(path, qframe::runtime::html_page(&fragments)).expect("review page written");
}

#[test]
fn a_scratch_machine_opens_on_its_applications() {
    let scratch = Scratch::new(
        "frame",
        &[Sample::new("firefox", "143.0-1", "Web browser").app(), Sample::new("bash", "5.3-1", "Shell").dependency()],
    );
    let recorded = Arc::new(Recorded::default());
    let mut h = Harness::with_env(app_in(&scratch, Settings::in_memory(), &recorded), env(), 120, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    let screen = h.screen();
    assert!(screen.contains("firefox") && !screen.contains("5.3-1"), "only the application:\n{screen}");
    assert!(screen.contains("1 app · 2 packages"), "{screen}");
}

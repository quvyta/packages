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

mod aur;
mod flatpak;
mod flow;
mod orphans;
mod polkit;
mod snap;
mod upgrade;

/// Built-in files plus the compiled-in locales and keymap, as the runtime loads them.
fn env() -> Env {
    crate::locales::env()
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
        appearance: crate::testing::appearance_apart(),
        snap_socket: &nowhere().join("snapd.socket"),
        first_run: None,
    };
    // Nothing of the machine running the tests is looked at: no snapshot tool, no unit folder.
    Qpackages::new(machine, &settings).with_places(Places {
        root: nowhere(),
        units: None,
        exe: None,
        runtime: None,
        home: None,
        cache: None,
        data: None,
    })
}

/// The calls a test's actions made, without those the application makes on its own when it
/// starts: the questions of which packages no repository offers and which are orphans, the
/// AUR's update check, the download of Arch's news, and Discover asking the network for its
/// rankings.
fn after_reads(recorded: &Recorded) -> Vec<Call> {
    recorded
        .calls()
        .into_iter()
        .filter(|call| !(["-Qqm", "-Qtdq", "-Qua"].contains(&call.args.join(" ").as_str()) || call.program == "curl"))
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
    Found { at: 1_789_999_320, repo: Ok(repo), aur: None, flatpak: None, snap: None }
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
    let mut h = harness(100, 32);
    let header = h.screen().lines().next().expect("a header").to_owned();
    let x = header.chars().position(|c| c == '▤').expect("the settings button");
    h.click(i32::try_from(x).unwrap(), 0);
    assert!(h.app().settings_open);
    let screen = h.screen();
    for text in ["Settings", "Back", "Sources", "Updates", "Backup"] {
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
fn the_settings_button_is_three_cells_and_brightens_under_the_pointer() {
    for mode in [GlyphMode::Unicode, GlyphMode::Nerd, GlyphMode::Ascii] {
        let mut h = harness(100, 24);
        h.set_glyph_mode(mode);
        let glyph = h.env().icons().glyph("settings").into_owned();
        let (x, y) = h.find(&glyph).unwrap_or_else(|| panic!("the settings button in {mode:?}:\n{}", h.screen()));
        let x = u16::try_from(x).expect("on screen");
        assert_eq!(y, 0, "in the header, {mode:?}");
        // A space, the glyph and a space, in every glyph mode; the whole three cells are the
        // target, and all three light together.
        let rest = [h.bg(x - 1, 0), h.bg(x, 0), h.bg(x + 1, 0)];
        h.hover(i32::from(x) - 1, 0);
        let lit = [h.bg(x - 1, 0), h.bg(x, 0), h.bg(x + 1, 0)];
        assert_ne!(lit, rest, "hovering the first space lights the button in {mode:?}");
        assert!(lit.iter().all(|cell| *cell == lit[0]), "all three cells light together in {mode:?}");
        h.click(i32::from(x) + 1, 0);
        assert!(h.app().settings_open, "the last of the three cells presses it in {mode:?}");
    }
}

#[test]
fn the_settings_button_says_what_it_does_when_the_keyboard_reaches_it() {
    let mut h = harness(100, 24);
    h.set_locale("tr");
    // The button carries its own words, so a lone glyph is never a mystery; the keyboard gets
    // them at once, without waiting for a pointer to rest.
    let said = |h: &Harness<Qpackages>| h.screen().lines().nth(1).unwrap_or_default().contains("Ayarlar");
    assert!(!said(&h), "nothing is said while nothing is focused:\n{}", h.screen());
    let mut reached = false;
    for _ in 0..60 {
        h.press("tab");
        if said(&h) {
            reached = true;
            break;
        }
    }
    assert!(reached, "the keyboard never reached the button, or it said nothing:\n{}", h.screen());
    h.press("enter");
    assert!(h.app().settings_open, "enter on the focused button opens the page");
}

#[test]
fn the_updates_tab_carries_the_number_of_waiting_updates() {
    let mut h = harness(120, 24);
    let quiet = h.screen().lines().next().unwrap_or_default().to_owned();
    assert!(!quiet.contains('3'), "no updates, no count:\n{quiet}");
    h.send(Msg::Updates(updates::Msg::Checked(three_updates())));
    let header = h.screen().lines().next().unwrap_or_default().to_owned();
    let updates = header.find("Updates").expect("the tab");
    let count = header[updates..].find('3').map(|at| updates + at).expect("the count");
    assert!(header[updates + "Updates".len()..count].trim().is_empty(), "the count follows its tab:\n{header}");
    // The count is part of the tab, a step quieter than its name.
    let at = |x: usize| h.fg(u16::try_from(x).expect("on screen"), 0);
    assert_ne!(at(count), at(updates), "the count is quieter than the name:\n{header}");
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

/// The words only each language can produce, on the screens they stand on: the name of the tab
/// that lists every package, the size column beside it, the button that looks for updates, and
/// the title of the settings page. None of them is the English word, so none of them can be
/// drawn by a file whose texts never reached the screen.
const OWN_WORDS: &[(&str, [&str; 4])] = &[
    ("en", ["All packages", "Size", "Check now", "Settings"]),
    ("tr", ["Bütün paketler", "Boyut", "Şimdi kontrol et", "Ayarlar"]),
    ("de", ["Alle Pakete", "Größe", "Jetzt suchen", "Einstellungen"]),
    ("es", ["Todos", "Tamaño", "Buscar ahora", "Ajustes"]),
    ("fr", ["Tous", "Taille", "Vérifier", "Réglages"]),
    ("pt-BR", ["Todos os pacotes", "Tamanho", "Verificar agora", "Configurações"]),
    ("ru", ["Все пакеты", "Размер", "Проверить сейчас", "Настройки"]),
    ("zh-Hans", ["全部软件包", "大小", "立即检查", "设置"]),
    ("ja", ["すべて", "サイズ", "今すぐ確認", "設定"]),
];

#[test]
fn every_language_shows_its_own_words() {
    // Counting keys says nothing about what reaches the screen: a file whose texts are all English
    // carries every key, keeps every placeholder and holds the right plural forms, so it passes
    // every other check and the screen still comes out English in all nine languages. Each
    // language is therefore asked for four words written out here rather than read from its own
    // file, because a word read from the file would be produced by the broken case just as well.
    assert_eq!(OWN_WORDS.len(), crate::locales::LOCALES.len(), "every language qpac speaks is asked for its own words");
    for (code, words) in OWN_WORDS {
        assert!(crate::locales::tests::codes().iter().any(|here| here == code), "`{code}` is not compiled in");
        let mut h = harness(120, 40);
        h.set_locale(code);
        let [all, size, check, settings] = *words;
        let installed = h.screen();
        for text in [all, size] {
            assert!(installed.contains(text), "`{code}` should say `{text}` on the Installed tab:\n{installed}");
        }
        h.send(Msg::Tab(Tab::Updates.index()));
        let updates = h.screen();
        assert!(updates.contains(check), "`{code}` should say `{check}` on the Updates tab:\n{updates}");
        h.send(Msg::OpenSettings);
        let page = h.screen();
        assert!(page.contains(settings), "`{code}` should say `{settings}` on the settings page:\n{page}");
    }
}

/// The fixed labels each page must show whole, by the page's own name. They are the words that
/// tell the user where they are and what a row is for: a cut heading leaves a settings section
/// nameless. Values, package names and the counts beside them are left out on purpose, because
/// the layout is allowed to shorten those with an ellipsis when the screen is narrow.
const LABELS: &[(&str, &[&str])] = &[
    ("discover", &["store.kind.label", "store.home.see-all", "store.catalog.install"]),
    ("installed", &["installed.apps", "installed.all", "packages.name", "packages.version", "packages.size"]),
    ("updates", &["updates.check-now", "updates.repo"]),
    (
        "settings",
        &[
            "settings-page.title",
            "settings-page.back",
            "settings-page.sources",
            "settings-page.aur-helper",
            "settings-page.updates",
            "settings-page.check",
            "settings-page.interval",
            "settings-page.backup",
            "settings-page.backup-tool",
            "settings-page.cleanup",
            "settings-page.orphans",
            "settings-page.mirrors",
            "settings-page.privilege",
            "settings-page.tool",
        ],
    ),
];

#[test]
fn every_page_shows_its_labels_whole_in_every_language_on_a_narrow_screen() {
    // Translations run longer than English, and a label the layout cannot fit is shortened with
    // an ellipsis rather than left out, so a screen that merely draws something is no proof. The
    // test therefore reads each label's text out of the language file and looks for it whole: a
    // shortened "Güncellemeler" is no longer the string the file holds, and the search fails.
    // The screen is tall, so nothing is missing for want of room below and width alone is what is
    // under test; the widths are the narrow ones qpac is already held to.
    for code in crate::locales::tests::codes() {
        for width in [36, 60, 80, 120] {
            let mut h = harness(width, 200);
            h.set_locale(&code);
            h.send(Msg::Updates(updates::Msg::Checked(three_updates())));
            h.send(Msg::Installed(installed::Msg::Select(0)));
            let pages = TABS.iter().map(|tab| (tab.key(), Msg::Tab(tab.index())));
            for (name, page) in pages.chain([("settings", Msg::OpenSettings)]) {
                h.send(page);
                let screen = h.screen();
                let keys = LABELS
                    .iter()
                    .find(|(page, _)| *page == name)
                    .map(|(_, keys)| *keys)
                    .unwrap_or_else(|| panic!("no labels are listed for the `{name}` page"));
                for key in keys {
                    let label = crate::locales::tests::label(&code, key);
                    assert!(
                        screen.contains(&label),
                        "`{key}` is cut off in `{code}` on {name} at {width} columns; it reads `{label}`:\n{screen}"
                    );
                }
            }
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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::env::Env;
use qframe::icons::GlyphMode;

use super::*;
use crate::helper::session::InProcess;
use crate::runner::Recorded;

mod flow;

/// Built-in files plus the compiled-in locales and keymap, as the runtime loads them.
fn env() -> Env {
    crate::test_env()
}

/// The recorded local database: bash, plus two records that cannot be read.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/localdb")
}

/// A pretend machine with pacman and paru but neither flatpak nor snap.
fn machine(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// The application on the pretend machine, running programs from `recorded` as user `uid`, with
/// transactions carried out by helpers from `helper`.
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
        lock_dir,
        lookup: Arc::new(machine),
        runner: Arc::clone(recorded) as Arc<dyn crate::runner::Runner>,
        helper: helper.start_fn(),
        uid: Some(1000),
    };
    Qpackages::new(Machine { uid, ..machine }, &settings)
}

fn app(settings: &str) -> Qpackages {
    let recorded = Arc::new(Recorded::default());
    app_on(settings, &recorded, &InProcess::new(&recorded, 0), Some(1000), &fixture())
}

fn harness(width: u16, height: u16) -> Harness<Qpackages> {
    let mut h = Harness::with_env(app(""), env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    h
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
fn the_screen_stands_before_the_first_read_and_fills_when_it_answers() {
    let mut h = Harness::with_env(Unread(app("")), env(), 100, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    let screen = h.screen();
    assert!(screen.contains("Reading the installed packages"), "the table says it is loading:\n{screen}");
    assert!(!screen.contains("No packages are installed"), "an empty table is not claimed:\n{screen}");
    assert!(!screen.contains("not installed"), "nothing is said about a source before looking:\n{screen}");
    assert!(screen.contains("Nothing selected"), "the detail panel stands:\n{screen}");
    h.send(Msg::Source(3));
    assert!(h.screen().contains("Looking for Snap"), "{}", h.screen());
    h.send(Msg::Source(0));
    let snapshot = h.app().0.reload.read();
    h.send(Msg::Reloaded(snapshot));
    let screen = h.screen();
    assert!(screen.contains("bash"), "the packages arrived:\n{screen}");
    assert!(!screen.contains("Reading"), "{screen}");
    h.send(Msg::Source(3));
    assert!(h.screen().contains("Snap is not installed"), "{}", h.screen());
    let mut unread = Harness::with_env(Unread(app("")), env(), 100, 24);
    unread.set_locale("tr");
    assert!(unread.screen().contains("Kurulu paketler okunuyor"), "{}", unread.screen());
}

#[test]
fn the_first_read_starts_from_init_and_the_first_frame_already_shows_it() {
    let h = harness(100, 24);
    assert!(h.app().sources.is_some(), "the read answered before the first frame was painted");
    assert!(h.screen().contains("bash"), "{}", h.screen());
}

#[test]
fn opens_on_the_installed_packages() {
    let h = harness(100, 24);
    let screen = h.screen();
    for text in ["qpac", "bash", "5.3.15-1", "9.6 MiB", "Explicit", "Pacman", "Snap", "not installed", "quit"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(screen.contains("2 records could not be read"), "the broken fixture records are counted:\n{screen}");
    assert!(screen.contains("Nothing selected"), "nothing is selected at first:\n{screen}");
}

#[test]
fn the_search_filters_the_table_and_says_when_nothing_matches() {
    let mut h = harness(100, 24);
    h.press("/");
    assert!(h.is_focused("search"), "the search key puts the keyboard in the field");
    h.type_text("ba");
    assert!(h.screen().contains("5.3.15-1"), "bash still matches:\n{}", h.screen());
    h.type_text("zinga");
    let screen = h.screen();
    assert!(!screen.contains("5.3.15-1"), "bash is filtered out:\n{screen}");
    assert!(screen.contains("No package matches the search"), "{screen}");
    assert_eq!(h.app().search, "bazinga");
}

#[test]
fn the_search_looks_at_the_description_too() {
    let mut h = harness(100, 24);
    h.send(Msg::Search("bourne".to_owned()));
    assert!(h.screen().contains("5.3.15-1"), "{}", h.screen());
}

#[test]
fn selecting_a_row_shows_its_details() {
    let mut h = harness(120, 30);
    h.click_text("5.3.15-1");
    let screen = h.screen();
    let expected = [
        "The GNU Bourne Again shell",
        "as a dependency",
        "2026-06-13",
        "GPL-3.0-or-later",
        "readline",
        "ncurses",
        "gnu.org",
    ];
    for text in expected {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Nothing selected"), "{screen}");
}

#[test]
fn the_selection_follows_the_package_through_a_search() {
    let mut h = harness(120, 30);
    h.send(Msg::Select(0));
    h.send(Msg::Search("nothing".to_owned()));
    assert_eq!(h.app().selected_row(), None, "the selected package is hidden");
    assert!(h.screen().contains("Nothing selected"), "{}", h.screen());
    h.send(Msg::Search(String::new()));
    assert_eq!(h.app().selected_row(), Some(0), "and comes back with its row");
    assert!(h.screen().contains("The GNU Bourne Again shell"), "{}", h.screen());
}

#[test]
fn a_check_survives_a_search_that_hides_the_row() {
    let mut h = harness(100, 24);
    h.send(Msg::Toggle(0));
    assert_eq!(h.app().marks, [true]);
    h.send(Msg::Search("nothing".to_owned()));
    assert_eq!(h.app().marks, []);
    h.send(Msg::Search(String::new()));
    assert_eq!(h.app().marks, [true]);
    h.send(Msg::Toggle(0));
    assert_eq!(h.app().marks, [false]);
    assert!(h.app().checked.is_empty());
}

#[test]
fn sorting_is_asked_of_the_application() {
    let mut h = harness(100, 24);
    h.send(Msg::Sort(packages::SIZE, SortDirection::Descending));
    assert_eq!(h.app().sort, (packages::SIZE, SortDirection::Descending));
    assert!(h.screen().contains("bash"), "{}", h.screen());
}

#[test]
fn a_missing_source_is_faint_and_says_it_is_not_installed() {
    let mut h = harness(100, 24);
    let (px, py) = h.find("Pacman").expect("pacman is listed");
    let (sx, sy) = h.find("Snap").expect("snap is listed");
    let pacman = h.fg(u16::try_from(px).unwrap(), u16::try_from(py).unwrap());
    let snap = h.fg(u16::try_from(sx).unwrap(), u16::try_from(sy).unwrap());
    assert_ne!(pacman, snap, "a missing source is drawn faint");
    h.click_text("Snap");
    let screen = h.screen();
    assert!(screen.contains("Snap is not installed"), "{screen}");
    assert!(!screen.contains("5.3.15-1"), "the table makes way for the message:\n{screen}");
    assert!(!screen.contains("Install"), "installing comes with its flow, not before:\n{screen}");
}

#[test]
fn a_source_this_version_does_not_manage_says_so() {
    let mut h = harness(100, 24);
    h.click_text("AUR");
    assert!(h.screen().contains("AUR comes in a later version"), "{}", h.screen());
}

#[test]
fn a_source_turned_off_in_the_settings_leaves_the_sidebar() {
    let app = app("[sources]\nsnap = false\n");
    let mut h = Harness::with_env(app, env(), 100, 24);
    h.set_locale("en");
    let screen = h.screen();
    assert!(!screen.contains("Snap"), "{screen}");
    assert!(screen.contains("Flatpak"), "{screen}");
}

#[test]
fn a_source_index_off_the_list_changes_nothing() {
    let mut h = harness(100, 24);
    h.send(Msg::Source(9)).send(Msg::Select(9)).send(Msg::Toggle(9));
    assert_eq!(h.app().source, Source::Pacman);
    assert_eq!(h.app().selected, None);
    assert_eq!(h.app().marks, [false]);
}

#[test]
fn turkish_uses_its_own_words() {
    let mut h = harness(100, 24);
    h.set_locale("tr");
    h.send(Msg::Select(0));
    let screen = h.screen();
    for text in ["İstenen", "Kurulma nedeni", "kurulu değil", "Kurulum tarihi", "Bağımlılıklar", "ara"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains('⟦'), "a key is missing in Turkish:\n{screen}");
}

#[test]
fn narrow_ascii_screens_keep_the_rules() {
    for (width, height) in [(60, 20), (80, 24), (120, 30)] {
        let mut h = harness(width, height);
        h.set_glyph_mode(GlyphMode::Ascii);
        h.send(Msg::Select(0));
        let screen = h.screen();
        assert!(screen.contains("bash"), "{width}x{height}:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
        }
        h.send(Msg::Source(3));
        assert!(h.screen().contains("not installed"), "{width}x{height}:\n{}", h.screen());
    }
}

#[test]
fn the_table_pane_keeps_room_for_the_details() {
    let mut h = harness(60, 20);
    h.send(Msg::Split(200)).send(Msg::Select(0));
    assert!(h.screen().contains("The GNU"), "the details keep their minimum width:\n{}", h.screen());
}

/// With `QUVYTA_REVIEW=1`, writes the screen in both languages, wide and narrow, to
/// `target/qpackages-review.html` in colour for a visual review.
#[test]
fn visual_review() {
    if std::env::var_os("QUVYTA_REVIEW").is_none() {
        return;
    }
    let mut fragments = Vec::new();
    for (width, height) in [(120, 30), (80, 24), (60, 20)] {
        for locale in ["en", "tr"] {
            let mut h = harness(width, height);
            h.set_locale(locale);
            h.send(Msg::Select(0));
            fragments.push(h.html(&format!("packages {locale} {width}x{height}")));
            println!("packages {locale} {width}x{height}\n{}", h.screen());
            h.send(Msg::Source(3));
            fragments.push(h.html(&format!("snap {locale} {width}x{height}")));
            println!("snap {locale} {width}x{height}\n{}", h.screen());
        }
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/qpackages-review.html");
    std::fs::write(path, qframe::runtime::html_page(&fragments)).expect("review page written");
}

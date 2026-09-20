//! Snap driven through the screen: searching the store, installing, removing, and the states a
//! machine can be in before any of that is possible.
//!
//! Nothing here runs snapd or the `snap` program. snapd is a socket of the test's own that answers
//! with the JSON snapd answered in a throwaway container ([`FakeSnapd`]), and the `snap` program is
//! the recorded runner playing back what it printed there. The helper is the real root-side loop on
//! a thread, reading the same recording.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::storage::Settings;
use qpackages_core::catalog::merge::Offer;
use qpackages_core::helper::{Refusal, Request};
use qpackages_core::snap::{self, Job, api};
use qpackages_core::sources::Source;

use crate::app::{Msg, Qpackages, Tab};
use crate::runner::Recorded;
use crate::testing::{FakeSnapd, Scratch, app_with};
use crate::{settings_page, store};

/// The snap installed and removed: the smallest one the measurement used.
const HELLO: &str = "hello-world";

/// What snapd or the `snap` program printed in the container, from the core's recordings.
fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/snap").join(name);
    std::fs::read_to_string(path).expect("the recording is readable")
}

/// A pretend machine with pacman, paru, fakeroot and snap.
fn with_snap(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot", "snap"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// A pretend machine without snap.
fn without_snap(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// A snapd that is up, knows nothing installed and finds `hello-world` for every search.
fn snapd(socket: &Path) -> FakeSnapd {
    let fake = FakeSnapd::start(socket);
    fake.answer(api::SYSTEM_INFO, &fixture("api-system-info.json"));
    fake.answer(api::SNAPS, "{\"result\":[]}");
    fake
}

/// The screen on a machine with Snap, `width` × `height`, in `language`, on Discover.
fn screen(
    recorded: &Arc<Recorded>,
    scratch: &Scratch,
    language: &str,
    width: u16,
    height: u16,
    lookup: fn(&str) -> Option<PathBuf>,
) -> Harness<Qpackages> {
    let settings = Settings::parse_str("settings.toml", "");
    let app = app_with(scratch, settings, recorded, lookup).on_tab(Tab::Discover);
    let mut h = Harness::with_env(app, crate::locales::env(), width, height);
    h.set_locale(language).set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.render();
    h
}

/// Lets the work after a confirmation run to its end and the toasts slide in.
fn settle(h: &mut Harness<Qpackages>) -> String {
    for _ in 0..6 {
        h.advance(Duration::from_millis(20));
    }
    h.screen()
}

/// Types `term` in the search box and lets the debounce pass and every source answer.
fn search(h: &mut Harness<Qpackages>, term: &str) -> String {
    h.send(Msg::Discover(store::Msg::Query(term.to_owned())));
    for _ in 0..8 {
        h.advance(Duration::from_millis(100));
    }
    h.screen()
}

/// Presses the dialog's own `label` button: the last one on the line of its buttons, which is the
/// line with `cancel` on it.
fn confirm(h: &mut Harness<Qpackages>, cancel: &str, label: &str) {
    let screen = h.screen();
    let (y, line) = screen
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(cancel) && line.contains('▌'))
        .last()
        .unwrap_or_else(|| panic!("no dialog buttons on screen:\n{screen}"));
    let start = line.rfind(label).unwrap_or_else(|| panic!("`{label}` is not beside `{cancel}`:\n{screen}"));
    let x = line[..start].chars().count();
    h.click(i32::try_from(x).expect("a column"), i32::try_from(y).expect("a row"));
}

/// Asks Discover for `request`, as its buttons and checked cards do.
fn ask(h: &mut Harness<Qpackages>, request: store::Request) {
    h.send(Msg::Discover(store::Msg::Request(request)));
}

fn snap_offer(name: &str) -> Offer {
    Offer { source: Source::Snap, package: name.to_owned() }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

/// Has the runner answer the `snap` call with `args` with `stdout` and `stderr`, ending with `code`.
fn snap_says(recorded: &Recorded, args: &[String], stdout: &str, stderr: &str, code: i32) {
    recorded.answer_full(snap::SNAP_PATH, args, stdout, stderr, code);
}

/// The calls that ran the `snap` program, as `snap arg arg…` lines.
fn snap_calls(recorded: &Recorded) -> Vec<String> {
    recorded.command_lines().into_iter().filter(|line| line.starts_with(snap::SNAP_PATH)).collect()
}

/// The screen's text with the lines joined, so a sentence the narrow dialog wraps still reads
/// whole when only its words matter.
fn words(screen: &str) -> String {
    screen.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words of each language the tests look for.
struct Words {
    cancel: &'static str,
    install: &'static str,
    remove: &'static str,
    install_title: &'static str,
    remove_title: &'static str,
    installed: &'static str,
    removed: &'static str,
    failed: &'static str,
    classic: &'static str,
    socket_off: &'static str,
    not_answering: &'static str,
    ready: &'static str,
    turn_on: &'static str,
    build: &'static str,
    more_step: &'static str,
    snapd_failed: &'static str,
    link_title: &'static str,
    link: &'static str,
}

const EN: Words = Words {
    cancel: "Cancel",
    install: "Install",
    remove: "Remove",
    install_title: "Install 1 snap?",
    remove_title: "Remove 1 snap?",
    installed: "Installed hello-world",
    removed: "Removed hello-world",
    failed: "ended with code 1",
    classic: "runs outside the sandbox",
    socket_off: "snapd is installed but not switched on",
    not_answering: "snapd is not answering",
    ready: "ready, snapd 2.77.1-1",
    turn_on: "Turn snapd on",
    build: "Build snapd",
    more_step: "1 more step follows",
    snapd_failed: "snapd ended with code 1",
    link_title: "Make the /snap link?",
    link: "Make the link",
};

const TR: Words = Words {
    cancel: "Vazgeç",
    install: "Kur",
    remove: "Kaldır",
    install_title: "1 snap kurulsun mu?",
    remove_title: "1 snap kaldırılsın mı?",
    installed: "hello-world kuruldu",
    removed: "hello-world kaldırıldı",
    failed: "1 koduyla bitti",
    classic: "korumalı alanın dışında çalışır",
    socket_off: "snapd kurulu ama açık değil",
    not_answering: "snapd yanıt vermiyor",
    ready: "hazır, snapd 2.77.1-1",
    turn_on: "snapd'yi aç",
    build: "snapd derle",
    more_step: "1 adım daha gelecek",
    snapd_failed: "snapd 1 koduyla bitti",
    link_title: "/snap bağı kurulsun mu?",
    link: "Bağı kur",
};

/// Every language and a narrow and a wide screen, as the other screen tests go.
fn layouts() -> [(&'static str, &'static Words, u16, u16); 4] {
    [("en", &EN, 120, 30), ("tr", &TR, 120, 30), ("en", &EN, 60, 24), ("tr", &TR, 60, 24)]
}

#[test]
fn a_search_finds_snaps_and_they_stand_as_cards_of_their_own() {
    for (language, _, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-search", &[]);
        let fake = snapd(&scratch.root().join("snapd.socket"));
        fake.answer(&api::find_path("hello"), &fixture("api-find-q.json"));
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        let shown = search(&mut h, "hello");
        assert!(shown.contains("Hello World"), "the snap's shown name in {language} at {width}:\n{shown}");
        assert!(fake.reads(&api::find_path("hello")) > 0, "{:?}", fake.asked());
        assert!(snap_calls(&recorded).is_empty(), "the socket answers; the program is never run");
    }
}

#[test]
fn a_snap_is_installed_through_the_helper_with_the_job_number_and_its_progress() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-install", &[]);
        let fake = snapd(&scratch.root().join("snapd.socket"));
        let start = snap::job_args(Job::Install, &names(&[HELLO]));
        snap_says(&recorded, &start, &fixture("install-no-wait.out"), &fixture("install-first.err"), 0);
        snap_says(&recorded, &snap::watch_args(10), "", "", 0);
        fake.answer(&api::change_path(10), &fixture("api-change-done.json"));
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        ask(&mut h, store::Request::Install(vec![snap_offer(HELLO)]));
        let shown = words(&h.screen());
        for text in [words_of.install_title, HELLO] {
            assert!(shown.contains(text), "`{text}` in {language} at {width}:\n{}", h.screen());
        }
        assert!(!shown.contains(words_of.classic), "this one is confined the usual way:\n{}", h.screen());
        assert!(snap_calls(&recorded).is_empty(), "nothing runs before yes");
        // Once it is installed snapd lists it, which is what marks the card.
        fake.answer(api::SNAPS, &fixture("api-snaps.json"));
        confirm(&mut h, words_of.cancel, words_of.install);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(words_of.installed), "the success toast in {language}:\n{screen}");
        assert_eq!(
            snap_calls(&recorded),
            [format!("{} install --no-wait -- {HELLO}", snap::SNAP_PATH), format!("{} watch 10", snap::SNAP_PATH),],
            "started without waiting, then waited for by its number"
        );
        assert!(fake.reads(&api::change_path(10)) > 0, "the job was followed on the socket: {:?}", fake.asked());
        assert!(h.handoffs().len() == 1, "the helper is asked for once: snapd refuses an ordinary user");
    }
}

#[test]
fn a_job_that_ends_badly_names_snapd_and_keeps_its_words_on_screen() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-job-failed", &[]);
        let fake = snapd(&scratch.root().join("snapd.socket"));
        let start = snap::job_args(Job::Install, &names(&["core22"]));
        snap_says(&recorded, &start, "11\n", &fixture("install-first.err"), 0);
        // snapd took the job and then it did not go through.
        snap_says(&recorded, &snap::watch_args(11), "", "", 1);
        fake.answer(&api::change_path(11), &fixture("api-change-doing.json"));
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        ask(&mut h, store::Request::Install(vec![snap_offer("core22")]));
        confirm(&mut h, words_of.cancel, words_of.install);
        let shown = settle(&mut h);
        assert!(words(&shown).contains(words_of.snapd_failed), "the failure names snapd in {language}:\n{shown}");
        assert!(!shown.contains("pacman"), "nothing speaks of pacman:\n{shown}");
        assert!(!shown.lines().any(|line| line.trim() == "11"), "the number is the answer, not a line:\n{shown}");
        if width >= 120 {
            // The narrow screen has no room for the pane; there the toast is the whole story.
            assert!(shown.contains("was not found in your $PATH"), "snapd's own words stay in the pane:\n{shown}");
        }
    }
}

#[test]
fn the_reminder_about_the_path_is_no_failure() {
    let recorded = Arc::new(Recorded::default());
    let scratch = Scratch::new("snap-path-warning", &[]);
    let fake = snapd(&scratch.root().join("snapd.socket"));
    let start = snap::job_args(Job::Install, &names(&[HELLO]));
    // snapd writes this to its error stream on every install; it says nothing about the result.
    snap_says(&recorded, &start, "10\n", &fixture("install-first.err"), 0);
    snap_says(&recorded, &snap::watch_args(10), "", "", 0);
    fake.answer(&api::change_path(10), &fixture("api-change-done.json"));
    let mut h = screen(&recorded, &scratch, "en", 120, 40, with_snap);
    settle(&mut h);
    ask(&mut h, store::Request::Install(vec![snap_offer(HELLO)]));
    fake.answer(api::SNAPS, &fixture("api-snaps.json"));
    confirm(&mut h, "Cancel", "Install");
    let shown = words(&settle(&mut h));
    assert!(shown.contains(EN.installed), "the install went through:\n{}", h.screen());
    assert!(!shown.contains("ended with code"), "nothing failed:\n{}", h.screen());
}

#[test]
fn a_classic_snap_says_what_it_means_and_the_snap_link_is_offered_first() {
    for (language, words_of, width, height) in layouts() {
        let classic = "test-snapd-classic-confinement";
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-classic", &[]);
        let fake = snapd(&scratch.root().join("snapd.socket"));
        fake.answer(&api::find_path(classic), &fixture("api-snaps.json"));
        let start = snap::job_args(Job::InstallClassic, &names(&[classic]));
        snap_says(&recorded, &start, "12\n", "", 0);
        snap_says(&recorded, &snap::watch_args(12), "", "", 0);
        fake.answer(&api::change_path(12), &fixture("api-change-done.json"));
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        // The search is what tells the page this snap runs outside the sandbox.
        search(&mut h, classic);
        ask(&mut h, store::Request::Install(vec![snap_offer(classic)]));
        // This machine has no /snap, and snapd refuses a classic snap without it, so the link is
        // asked for first rather than after a confirmation that could not have run.
        let shown = words(&h.screen());
        assert!(shown.contains(words_of.link_title), "the link comes first in {language}:\n{}", h.screen());
        assert!(shown.contains(words_of.more_step), "and the installation follows it:\n{shown}");
        confirm(&mut h, words_of.cancel, words_of.link);
        let shown = words(&settle(&mut h));
        assert!(shown.contains(words_of.classic), "then the warning in {language}:\n{}", h.screen());
        confirm(&mut h, words_of.cancel, words_of.install);
        settle(&mut h);
        assert_eq!(
            snap_calls(&recorded).into_iter().filter(|line| line.contains(" install ")).collect::<Vec<_>>(),
            [format!("{} install --classic --no-wait -- {classic}", snap::SNAP_PATH)],
            "`--classic` only where the confirmation said so"
        );
        assert!(
            std::fs::read_link(scratch.root().join("snap")).is_ok(),
            "the helper made the link under the machine's root"
        );
    }
}

#[test]
fn with_the_link_already_there_a_classic_install_is_one_step() {
    let classic = "test-snapd-classic-confinement";
    let recorded = Arc::new(Recorded::default());
    let scratch = Scratch::new("snap-classic-linked", &[]);
    let fake = snapd(&scratch.root().join("snapd.socket"));
    fake.answer(&api::find_path(classic), &fixture("api-snaps.json"));
    // The link is where snapd wants it already.
    std::fs::create_dir_all(scratch.root().join("var/lib/snapd/snap")).expect("snapd's own folder");
    std::os::unix::fs::symlink(scratch.root().join("var/lib/snapd/snap"), scratch.root().join("snap"))
        .expect("the link");
    let start = snap::job_args(Job::InstallClassic, &names(&[classic]));
    snap_says(&recorded, &start, "12\n", "", 0);
    snap_says(&recorded, &snap::watch_args(12), "", "", 0);
    fake.answer(&api::change_path(12), &fixture("api-change-done.json"));
    let mut h = screen(&recorded, &scratch, "en", 120, 30, with_snap);
    settle(&mut h);
    search(&mut h, classic);
    ask(&mut h, store::Request::Install(vec![snap_offer(classic)]));
    let shown = words(&h.screen());
    assert!(shown.contains(EN.classic), "the warning is the first thing asked:\n{}", h.screen());
    assert!(!shown.contains(EN.more_step), "nothing comes before or after it:\n{shown}");
}

#[test]
fn a_snap_is_removed_through_the_helper_and_the_card_follows() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-remove", &[]);
        let fake = snapd(&scratch.root().join("snapd.socket"));
        fake.answer(api::SNAPS, &fixture("api-snaps.json"));
        let start = snap::job_args(Job::Remove, &names(&[HELLO]));
        snap_says(&recorded, &start, "13\n", "", 0);
        snap_says(&recorded, &snap::watch_args(13), "", "", 0);
        fake.answer(&api::change_path(13), &fixture("api-change-done.json"));
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        ask(&mut h, store::Request::Remove(vec![snap_offer(HELLO)]));
        let shown = words(&h.screen());
        for text in [words_of.remove_title, HELLO] {
            assert!(shown.contains(text), "`{text}` in {language} at {width}:\n{}", h.screen());
        }
        fake.answer(api::SNAPS, "{\"result\":[]}");
        confirm(&mut h, words_of.cancel, words_of.remove);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(words_of.removed), "the success toast in {language}:\n{screen}");
        assert_eq!(
            snap_calls(&recorded),
            [format!("{} remove --no-wait -- {HELLO}", snap::SNAP_PATH), format!("{} watch 13", snap::SNAP_PATH)]
        );
    }
}

#[test]
fn removing_a_snap_this_machine_does_not_have_runs_nothing() {
    let recorded = Arc::new(Recorded::default());
    let scratch = Scratch::new("snap-remove-none", &[]);
    let _fake = snapd(&scratch.root().join("snapd.socket"));
    let mut h = screen(&recorded, &scratch, "en", 120, 30, with_snap);
    settle(&mut h);
    ask(&mut h, store::Request::Remove(vec![snap_offer("no-such-snap")]));
    let screen = settle(&mut h);
    assert!(!screen.contains(EN.remove_title), "nothing to confirm:\n{screen}");
    assert!(snap_calls(&recorded).is_empty(), "snapd calls that one an answer with code 0, not a failure");
}

#[test]
fn an_install_that_snapd_refuses_keeps_its_words_on_screen() {
    for (language, words_of, width, height) in layouts() {
        let missing = "no-such-snap-qpac-xyz";
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-failed", &[]);
        let fake = snapd(&scratch.root().join("snapd.socket"));
        let start = snap::job_args(Job::Install, &names(&[missing]));
        snap_says(&recorded, &start, "", &fixture("install-missing.err"), 1);
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        ask(&mut h, store::Request::Install(vec![snap_offer(missing)]));
        confirm(&mut h, words_of.cancel, words_of.install);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(words_of.failed), "the failure in {language}:\n{screen}");
        assert!(screen.contains("not found"), "snapd's own error stays in the pane:\n{screen}");
        assert_eq!(fake.reads(&api::change_path(0)), 0, "there is no job to follow");
        assert!(
            !snap_calls(&recorded).iter().any(|line| line.contains(" watch ")),
            "nothing is waited for: snapd gave no number"
        );
    }
}

#[test]
fn the_settings_say_snapd_is_not_switched_on_and_offer_to_switch_it() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-socket-off", &[]);
        // No socket at all: the package leaves snapd.socket disabled.
        let unit = snap::SOCKET_UNIT;
        recorded.answer_full("/usr/bin/systemctl", &socket_args(true), "", "", 0);
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        h.send(Msg::OpenSettings);
        let screen = h.screen();
        assert!(words(&screen).contains(words_of.socket_off), "the row in {language} at {width}:\n{screen}");
        assert!(words(&screen).contains(words_of.turn_on), "and the offer:\n{screen}");
        h.send(Msg::Settings(settings_page::Msg::SnapSocket(true)));
        let shown = words(&h.screen());
        assert!(shown.contains(unit), "the confirmation names the unit:\n{}", h.screen());
        assert!(shown.contains(words_of.more_step), "the /snap link follows it:\n{shown}");
    }
}

#[test]
fn the_settings_say_snapd_is_not_answering_when_its_socket_is_dead() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-mute", &[]);
        // A file where the socket belongs: connecting is refused, the way a stopped snapd's is.
        std::fs::write(scratch.root().join("snapd.socket"), "").expect("the scratch folder takes a file");
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        h.send(Msg::OpenSettings);
        let screen = h.screen();
        assert!(words(&screen).contains(words_of.not_answering), "the row in {language} at {width}:\n{screen}");
        assert!(words(&screen).contains(words_of.turn_on), "the same offer starts it again:\n{screen}");
    }
}

#[test]
fn without_snapd_discover_shows_snap_inactive_with_an_offer_and_asks_it_of_nothing() {
    // Only the wide screens: the narrow one folds the kinds and the sources into one control.
    for (language, _, width, height) in layouts().into_iter().filter(|(_, _, width, _)| *width >= 120) {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-inactive", &[]);
        // No socket and no program: a source that is not installed is a state, not an error.
        let mut h = screen(&recorded, &scratch, language, width, height, without_snap);
        settle(&mut h);
        let shown = h.screen();
        let snap_row = shown
            .lines()
            .find(|line| line.contains("Snap"))
            .unwrap_or_else(|| panic!("Snap is listed among the sources in {language}:\n{shown}"));
        assert!(!snap_row.contains('✓'), "it is not ticked like a source that is there:\n{snap_row}");
        assert!(snap_calls(&recorded).is_empty(), "nothing is asked of a snapd that is not installed");
        // Snap is the fourth source, and its row leads to the settings, where the offer to build
        // snapd is.
        h.send(Msg::Discover(store::Msg::SourceRow(3)));
        settle(&mut h);
        assert!(h.app().settings_open(), "the row opens the settings:\n{}", h.screen());
    }
}

#[test]
fn the_settings_offer_to_build_snapd_from_the_aur_where_it_is_not_installed() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-not-installed", &[]);
        let mut h = screen(&recorded, &scratch, language, width, height, without_snap);
        settle(&mut h);
        h.send(Msg::OpenSettings);
        let shown = h.screen();
        assert!(words(&shown).contains(words_of.build), "snapd is built from the AUR in {language}:\n{shown}");
        assert!(!words(&shown).contains(words_of.turn_on), "nothing to switch on yet:\n{shown}");
    }
}

#[test]
fn the_settings_say_snapds_version_where_it_answers() {
    for (language, words_of, width, height) in layouts() {
        let recorded = Arc::new(Recorded::default());
        let scratch = Scratch::new("snap-settings-ready", &[]);
        let _fake = snapd(&scratch.root().join("snapd.socket"));
        let mut h = screen(&recorded, &scratch, language, width, height, with_snap);
        settle(&mut h);
        h.send(Msg::OpenSettings);
        let shown = h.screen();
        assert!(words(&shown).contains(words_of.ready), "snapd's version in {language} at {width}:\n{shown}");
        assert!(!words(&shown).contains(words_of.turn_on), "nothing to switch on:\n{shown}");
    }
}

/// The systemctl arguments the helper runs for snapd's socket.
pub(super) fn socket_args(on: bool) -> Vec<String> {
    Request::SnapdSocket(on).command().expect("the socket is switched with systemctl").1
}

#[test]
fn the_helper_refuses_a_name_that_breaks_snapds_rule_and_runs_nothing() {
    for name in ["Hello", "hello_world", "a", "-x", "x-", "a--b", "../evil", "--classic"] {
        assert_eq!(
            Request::parse(&format!("snap install {name}")),
            Err(Refusal::SnapName),
            "`{name}` never reaches snapd"
        );
    }
    for name in [HELLO, "core22", "test-snapd-classic-confinement"] {
        assert_eq!(Request::parse(&format!("snap install {name}")), Ok(Request::Snap(Job::Install, names(&[name]))));
    }
}

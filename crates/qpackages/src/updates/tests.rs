//! The Updates tab: the check run through the whole application on a pretend machine, and the
//! tab's states drawn on their own.

use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::storage::Settings;
use qpackages_core::catalog::flatpak::FLATPAK;
use qpackages_core::catalog::net::{CURL, curl_args};
use qpackages_core::flatpak::{self, Scope};
use qpackages_core::news::NEWS_URL;
use qpackages_core::pacman::command::{self, FAKEROOT, PACMAN};

use super::*;
use crate::app::{Msg as AppMsg, Qpackages};
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, app_in};

fn update(name: &str, from: &str, to: &str) -> Update {
    Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
}

/// 14:02 UTC on a day in 2026.
const AT: i64 = 1_789_999_320;

/// Two repository updates, one of them the kernel, and one from the AUR.
fn found() -> Found {
    Found {
        at: AT,
        repo: Ok(vec![update("linux", "6.18.1-1", "6.18.2-1"), update("mesa", "25.2.3-1", "25.2.4-1")]),
        aur: Some(Ok(vec![update("visual-studio-code-bin", "1.104.0-1", "1.105.0-1")])),
        flatpak: None,
        snap: None,
    }
}

/// The tab alone, fed messages as the application would, so a running check can be held still.
struct Tab(Updates, Cx);

impl App for Tab {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        let _ = self.0.update(msg);
        Command::none()
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        self.0.view(ui, self.1);
    }
}

fn tab(width: u16, height: u16) -> Harness<Tab> {
    let cx = Cx { aur: true, utc_offset: 0, can_check: true, busy: false, backup: backup::Plan::Off };
    let mut h = Harness::with_env(Tab(Updates::default(), cx), crate::locales::env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h
}

#[test]
fn updates_are_grouped_by_source_with_the_restart_note_on_the_kernel() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(found()));
    let screen = h.screen();
    let linux = format!("{:22}  {:9}  →  6.18.2-1", "linux", "6.18.1-1");
    for text in [
        "3 updates · last checked 14:02",
        "Check now",
        "Repositories",
        "AUR",
        linux.as_str(),
        "visual-studio-code-bin  1.104.0-1  →  1.105.0-1",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let linux = screen.lines().find(|line| line.contains("linux")).expect("the kernel's row");
    assert!(linux.contains("restart needed"), "{linux}");
    let mesa = screen.lines().find(|line| line.contains("mesa")).expect("mesa's row");
    assert!(!mesa.contains("restart"), "{mesa}");
    let repo = screen.find("Repositories").expect("a heading");
    assert!(repo < screen.find("AUR").expect("a heading"), "the repositories come first:\n{screen}");
    let top = screen.lines().find(|line| line.contains("Check now")).expect("the status line");
    assert!(top.contains("Update all"), "the update sits beside the check:\n{screen}");
}

#[test]
fn an_empty_group_is_left_out_and_a_held_back_package_is_not_counted() {
    let mut h = tab(100, 20);
    let mut held = update("linux", "6.18.1-1", "6.18.2-1");
    held.ignored = true;
    h.send(Msg::Checked(Found { at: AT, repo: Ok(vec![held]), aur: Some(Ok(Vec::new())), flatpak: None, snap: None }));
    let screen = h.screen();
    assert!(screen.contains("0 updates"), "{screen}");
    assert!(screen.contains("held back by IgnorePkg"), "{screen}");
    assert!(!screen.contains("AUR"), "the AUR has nothing, so it has no heading:\n{screen}");
    assert_eq!(h.app().0.pending(true), 0);
}

#[test]
fn nothing_to_update_says_so_with_the_time_of_the_check() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(Found { at: AT, repo: Ok(Vec::new()), aur: None, flatpak: None, snap: None }));
    let screen = h.screen();
    for text in ["Everything is up to date", "Last checked 14:02.", "Check now"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.set_locale("tr");
    let screen = h.screen();
    for text in ["Her şey güncel", "Son kontrol 14:02.", "Şimdi kontrol et"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn the_list_stays_while_a_check_runs_and_a_failed_check_keeps_it() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(found()));
    h.send(Msg::CheckNow);
    let screen = h.screen();
    assert!(screen.contains("linux") && screen.contains("checking"), "{screen}");
    let failure = Failure::Said("error: failed retrieving file 'core.db'".to_owned());
    h.send(Msg::Checked(Found { at: AT + 600, repo: Err(failure), aur: None, flatpak: None, snap: None }));
    let screen = h.screen();
    assert!(screen.contains("linux"), "the list stays:\n{screen}");
    assert!(screen.contains("The last check did not finish: error: failed retrieving file"), "{screen}");
    assert!(screen.contains("last checked 14:02"), "the time is the last check that got through:\n{screen}");
    assert!(!screen.contains("visual-studio-code-bin"), "the AUR was not asked this time:\n{screen}");
}

#[test]
fn a_second_check_does_not_start_while_one_runs() {
    let mut updates = Updates::default();
    assert_eq!(updates.update(Msg::CheckNow), Some(Request::Check));
    assert_eq!(updates.update(Msg::CheckNow), None);
    let _ = updates.update(Msg::Checked(found()));
    assert_eq!(updates.update(Msg::CheckNow), Some(Request::Check), "once it ended, another may start");
}

#[test]
fn a_first_check_that_fails_says_why_and_offers_another() {
    let mut h = tab(100, 20);
    let screen = h.screen();
    assert!(screen.contains("Looking for updates"), "before any answer:\n{screen}");
    h.send(Msg::Checked(Found { at: AT, repo: Err(Failure::NoFakeroot), aur: None, flatpak: None, snap: None }));
    let screen = h.screen();
    for text in ["Could not check for updates", "fakeroot", "base-devel", "Check now"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.click_text("Check now");
    assert!(h.screen().contains("Looking for updates"), "the check runs again:\n{}", h.screen());
}

#[test]
fn a_turned_off_aur_is_neither_shown_nor_counted() {
    let cx = Cx { aur: false, utc_offset: 0, can_check: true, busy: false, backup: backup::Plan::Off };
    let mut h = Harness::with_env(Tab(Updates::default(), cx), crate::locales::env(), 100, 20);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
    h.send(Msg::Checked(found()));
    let screen = h.screen();
    assert!(screen.contains("2 updates") && !screen.contains("visual-studio"), "{screen}");
    assert_eq!(h.app().0.pending(false), 2);
    assert_eq!(h.app().0.pending(true), 3);
}

#[test]
fn the_arrow_follows_the_glyph_mode_and_ascii_keeps_the_rules() {
    for (width, height) in [(36, 14), (60, 20), (100, 24)] {
        let mut h = tab(width, height);
        h.send(Msg::Checked(found()));
        h.set_glyph_mode(GlyphMode::Nerd);
        assert!(h.screen().contains('\u{f061}'), "{}", h.screen());
        h.set_glyph_mode(GlyphMode::Ascii);
        let screen = h.screen();
        let kernel = screen.lines().find(|line| line.contains("linux")).expect("the kernel's row");
        assert!(
            kernel.contains("6.18.1-1") && kernel.contains(">") && kernel.contains("6.18.2-1"),
            "never cut:\n{screen}"
        );
        assert_eq!(kernel.contains("restart needed"), width >= 100, "the note only where it fits:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|', '→'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
        }
        assert!(!screen.contains("-->"), "{screen}");
    }
}

#[test]
fn times_are_written_where_the_machine_stands() {
    assert_eq!(clock(AT, 0), "14:02");
    assert_eq!(clock(AT, 180), "17:02", "Istanbul");
    assert_eq!(clock(AT, -900), "23:02", "an offset can cross midnight");
}

/// The application on a pretend machine whose repositories have the kernel and mesa waiting,
/// and the AUR a code editor, as pacman and paru would print them.
fn machine(settings: &str) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("updates", &[Sample::new("linux", "6.18.1-1", "The Linux kernel")]);
    scratch.repository("core");
    let work = scratch.check();
    let recorded = Arc::new(Recorded::default());
    recorded.answer(FAKEROOT, &command::refresh(&work), "", 0);
    recorded.answer(
        PACMAN,
        &command::update_check(&work),
        "linux 6.18.1-1 -> 6.18.2-1\nmesa 25.2.3-1 -> 25.2.4-1\n",
        0,
    );
    recorded.answer("paru", &command::aur_update_check(), "visual-studio-code-bin 1.104.0-1 -> 1.105.0-1\n", 0);
    let settings = Settings::parse_str("packages.conf", settings);
    let mut h = Harness::with_env(app_in(&scratch, settings, &recorded), crate::locales::env(), 120, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.advance(Duration::from_millis(20));
    (h, scratch, recorded)
}

#[test]
fn the_application_checks_once_it_knows_its_sources_and_counts_on_the_tab() {
    let (mut h, scratch, recorded) = machine("");
    let header = h.screen().lines().next().unwrap_or_default().to_owned();
    let updates = header.find("Updates").expect("the tab");
    assert!(header[updates..].contains('3'), "the count stands on the Updates tab:\n{header}");
    h.send(AppMsg::Tab(crate::app::Tab::Updates.index()));
    let screen = h.screen();
    for text in ["3 updates · last checked", "Repositories", "linux", "mesa", "AUR", "visual-studio-code-bin"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let lines = recorded.command_lines();
    assert!(!lines.iter().any(|line| line.contains("-Syu") || line.starts_with("pacman -Sy")), "{lines:?}");
    assert!(
        lines.iter().any(|line| line.starts_with("fakeroot -- pacman -Sy --dbpath") && line.contains("check")),
        "only the private copy is refreshed: {lines:?}"
    );
    assert!(scratch.check().join("sync/core.db").exists(), "the copy is where qpac keeps it");
    h.click_text("Check now");
    h.advance(Duration::from_millis(20));
    let refreshes = recorded.command_lines().iter().filter(|line| line.starts_with("fakeroot")).count();
    assert_eq!(refreshes, 2, "the button checks again");
}

#[test]
fn a_turned_off_aur_is_not_asked() {
    let (mut h, _scratch, recorded) = machine("[sources]\naur = false\n");
    h.send(AppMsg::Tab(crate::app::Tab::Updates.index()));
    let screen = h.screen();
    assert!(screen.contains("2 updates") && !screen.contains("visual-studio"), "{screen}");
    assert!(!recorded.command_lines().iter().any(|line| line.starts_with("paru")), "{:?}", recorded.command_lines());
}

/// Two news items of the last days: one asking for the user's hand, one not.
fn news() -> Vec<NewsItem> {
    let item = |title: &str, published: i64, manual_intervention: bool| NewsItem {
        title: title.to_owned(),
        link: "https://archlinux.org/news/".to_owned(),
        published,
        manual_intervention,
    };
    vec![
        item("virtualbox-ext-vnc >= 7.2.12-2 requires manual intervention", AT - 86_400, true),
        item("Arch Linux 2026 Leader Election Results", AT - 3 * 86_400, false),
    ]
}

#[test]
fn recent_news_stands_above_the_list_in_every_glyph_mode_and_narrow() {
    for (width, height) in [(40, 16), (100, 20)] {
        let mut h = tab(width, height);
        h.send(Msg::Checked(found()));
        h.send(Msg::News(Ok(news())));
        for mode in [GlyphMode::Unicode, GlyphMode::Ascii, GlyphMode::Nerd] {
            h.set_glyph_mode(mode);
            let screen = h.screen();
            let manual = screen.lines().position(|line| line.contains("2026-09-20  virtualbox")).expect("the item");
            let list = screen.lines().position(|line| line.contains("Repositories")).expect("the list");
            assert!(manual < list, "the news comes first:\n{screen}");
            assert!(screen.contains("2026-09-18  Arch Linux 2026"), "{mode:?} at {width}:\n{screen}");
            for forbidden in ['[', ']', '{', '}', '|'] {
                assert!(!screen.contains(forbidden), "`{forbidden}` at {width}:\n{screen}");
            }
        }
        if width >= 100 {
            assert!(h.screen().contains("requires manual intervention · needs your hand"), "{}", h.screen());
            h.set_locale("tr");
            assert!(h.screen().contains("elle müdahale"), "{}", h.screen());
        }
    }
}

#[test]
fn news_that_could_not_be_read_is_one_quiet_line() {
    let mut h = tab(100, 20);
    h.send(Msg::Checked(found()));
    h.send(Msg::News(Err("Could not resolve host: archlinux.org".to_owned())));
    let screen = h.screen();
    assert!(screen.contains("Arch news could not be read."), "{screen}");
    assert!(!screen.contains("resolve host"), "the reason is not the user's business here:\n{screen}");
    h.send(Msg::News(Ok(Vec::new())));
    assert!(!h.screen().contains("Arch news"), "nothing recent takes no room:\n{}", h.screen());
}

#[test]
fn update_all_asks_for_the_repositories_updates_without_those_held_back() {
    let mut updates = Updates::default();
    let mut found = found();
    if let Ok(repo) = &mut found.repo {
        repo.push(Update { ignored: true, ..update("glibc", "2.42-1", "2.42-2") });
    }
    let _ = updates.update(Msg::Checked(found));
    let Some(Request::UpdateAll(list, snaps)) = updates.update(Msg::UpdateAll) else {
        panic!("an update is asked for");
    };
    let names: Vec<&str> = list.iter().map(|update| update.name.as_str()).collect();
    assert_eq!(names, ["linux", "mesa"], "pacman holds glibc back and the AUR is not built here");
    assert_eq!(snaps, Vec::<String>::new(), "snapd was not asked");
}

#[test]
fn snaps_with_a_newer_version_are_their_own_group_and_their_own_step() {
    let mut updates = Updates::default();
    let mut found = found();
    found.snap = Some(Ok(vec![update("hello", "2.9", "2.10")]));
    let _ = updates.update(Msg::Checked(found));
    assert_eq!(updates.pending(true), 4, "the snap counts on the tab like the rest");
    assert_eq!(updates.refreshable(), ["hello"]);
    let Some(Request::UpdateAll(list, snaps)) = updates.update(Msg::UpdateAll) else {
        panic!("an update is asked for");
    };
    assert_eq!(snaps, ["hello"], "snapd's part is asked for by name");
    assert!(!list.iter().any(|update| update.name == "hello"), "and never handed to pacman");
}

#[test]
fn the_feed_is_read_with_curl_and_the_recent_manual_item_reaches_the_tab() {
    let (mut h, _scratch, recorded) = machine("");
    let xml = "<rss><channel><item><title>foo &gt;= 2 requires manual intervention</title>\
               <link>https://archlinux.org/news/foo/</link>\
               <pubDate>Thu, 01 Jan 2099 00:00:00 +0000</pubDate></item></channel></rss>";
    recorded.answer(CURL, &curl_args(NEWS_URL), xml, 0);
    h.send(AppMsg::Tab(crate::app::Tab::Updates.index()));
    h.click_text("Check now");
    h.advance(Duration::from_millis(20));
    let screen = h.screen();
    assert!(screen.contains("2099-01-01  foo >= 2 requires manual intervention"), "{screen}");
    let curl = recorded.calls().into_iter().find(|call| call.program == CURL).expect("curl ran");
    assert!(curl.args.contains(&"=https".to_owned()), "only https: {:?}", curl.args);
}

#[test]
fn flatpak_updates_are_their_own_group_listed_but_not_offered() {
    let mut h = tab(100, 20);
    let mut found = found();
    found.flatpak = Some(Ok(vec![update("net.sourceforge.ExtremeTuxRacer", "0.8.4", "8f72400b6553")]));
    h.send(Msg::Checked(found.clone()));
    let screen = h.screen();
    for text in ["4 updates", "Flatpak", "net.sourceforge.ExtremeTuxRacer", "8f72400b6553", "use flatpak update"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let mut updates = Updates::default();
    let _ = updates.update(Msg::Checked(found));
    let Some(Request::UpdateAll(list, snaps)) = updates.update(Msg::UpdateAll) else {
        panic!("an update is asked for");
    };
    assert!(!list.iter().any(|update| update.name.starts_with("net.")), "Flatpak is not handed to pacman");
    assert!(snaps.is_empty(), "nor to snapd");
}

#[test]
fn a_flatpak_installation_that_did_not_answer_is_one_quiet_line_beside_the_rest() {
    let reason = "error: Remote \"no-such-remote\" not found in the user installation";
    let mut h = tab(120, 20);
    let mut found = found();
    found.flatpak = Some(Err(Failure::Partial {
        updates: vec![update("org.freedesktop.Platform", "freedesktop-sdk-25.08.16", "d27f7a6a974e")],
        failure: Box::new(Failure::Said(reason.to_owned())),
    }));
    h.send(Msg::Checked(found));
    let screen = h.screen();
    for text in ["org.freedesktop.Platform", "Flatpak could not be asked: error: Remote"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let headers = screen.lines().filter(|line| line.trim_end().ends_with("Flatpak")).count();
    assert_eq!(headers, 1, "the failure stands under the rows' own heading:\n{screen}");

    let mut h = tab(120, 20);
    let mut found = super::tests::found();
    found.flatpak = Some(Err(Failure::Said(reason.to_owned())));
    h.send(Msg::Checked(found));
    let screen = h.screen();
    assert!(screen.contains("Flatpak could not be asked"), "{screen}");
    assert!(!screen.contains("use flatpak update"), "no note without rows:\n{screen}");
}

/// The programs of a pretend machine that also has Flatpak.
fn with_flatpak(program: &str) -> Option<std::path::PathBuf> {
    crate::testing::programs(program)
        .or_else(|| (program == FLATPAK).then(|| std::path::Path::new("/usr/bin").join(program)))
}

fn flatpak_fixture(name: &str) -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/flatpak").join(name);
    std::fs::read_to_string(path).expect("the recording is readable")
}

/// [`machine`] with Flatpak installed, its user installation having two refs waiting as the
/// throwaway Arch container's Flatpak 1.18.3 printed them.
fn flatpak_machine(settings: &str) -> (Harness<Qpackages>, Scratch, Arc<Recorded>) {
    let scratch = Scratch::new("updates-flatpak", &[Sample::new("linux", "6.18.1-1", "The Linux kernel")]);
    scratch.repository("core");
    let work = scratch.check();
    let recorded = Arc::new(Recorded::default());
    recorded.answer(FAKEROOT, &command::refresh(&work), "", 0);
    recorded.answer(PACMAN, &command::update_check(&work), "linux 6.18.1-1 -> 6.18.2-1\n", 0);
    recorded.answer(FLATPAK, &flatpak::updates_args(Scope::User), &flatpak_fixture("updates-user.out"), 0);
    recorded.answer(FLATPAK, &flatpak::installed_args(Scope::User), &flatpak_fixture("list-user.out"), 0);
    recorded.answer(FLATPAK, &flatpak::updates_args(Scope::System), "", 0);
    recorded.answer(FLATPAK, &flatpak::installed_args(Scope::System), "", 0);
    let settings = Settings::parse_str("packages.conf", settings);
    let app = crate::testing::app_with(&scratch, settings, &recorded, with_flatpak);
    let mut h = Harness::with_env(app, crate::locales::env(), 120, 24);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.advance(Duration::from_millis(20));
    (h, scratch, recorded)
}

#[test]
fn the_application_asks_flatpak_for_updates_when_it_is_installed_and_on() {
    let (mut h, _scratch, recorded) = flatpak_machine("");
    h.click_text("Updates");
    let screen = h.screen();
    for text in ["3 updates", "Flatpak", "net.sourceforge.ExtremeTuxRacer", "org.freedesktop.Platform"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let lines = recorded.command_lines();
    assert!(lines.iter().any(|line| line.starts_with("flatpak --system remote-ls --updates")), "{lines:?}");
    h.click_text("Update all");
    let screen = h.screen();
    // Three updates are listed, and the confirmation asks about pacman's one alone.
    assert!(screen.contains("Update 1 package?"), "Flatpak's part is left to flatpak update:\n{screen}");
}

#[test]
fn a_turned_off_flatpak_is_not_asked_for_updates() {
    let (mut h, _scratch, recorded) = flatpak_machine("[sources]\nflatpak = false\n");
    h.click_text("Updates");
    let screen = h.screen();
    assert!(screen.contains("1 update") && !screen.contains("ExtremeTuxRacer"), "{screen}");
    let lines = recorded.command_lines();
    assert!(!lines.iter().any(|line| line.contains("remote-ls")), "{lines:?}");
}

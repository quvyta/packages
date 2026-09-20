//! Flatpak's installs and removals driven through the screen. Nothing here runs flatpak: every
//! answer is what flatpak 1.18 printed in a container, played back by the recorded runner, and the
//! helper is the real root-side loop on a thread, playing from the same recording.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::ProcessOutcome;
use qframe::storage::Settings;
use qpackages_core::catalog::flatpak::list_args;
use qpackages_core::catalog::merge::Offer;
use qpackages_core::flatpak::{self, FLATPAK_PATH};
use qpackages_core::helper::PACMAN_PATH;
use qpackages_core::pacman::command::{PACMAN, install, print_install};
use qpackages_core::sources::Source;

use crate::app::{Msg, Qpackages, Tab};
use crate::runner::Recorded;
use crate::testing::{Scratch, app_with};
use crate::{settings_page, store};

/// The id installed and removed: the small game the measurement used.
const TUX: &str = "net.sourceforge.ExtremeTuxRacer";

/// What flatpak printed in the container, from the core's recordings.
fn recording(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/flatpak").join(name);
    std::fs::read_to_string(path).expect("the recording is readable")
}

/// The lines of a recording, as the runner hands them on.
fn lines(name: &str) -> Vec<String> {
    recording(name).lines().map(str::to_owned).collect()
}

/// A pretend machine with pacman, paru, fakeroot and flatpak.
fn with_flatpak(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot", "flatpak"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// Says `listed` is what `flatpak list` finds from now on: lines of an id and an installation.
fn list(recorded: &Recorded, listed: &str) {
    recorded.answer("flatpak", &list_args(), listed, 0);
}

/// A runner on which the user has Flathub and nothing is installed from it yet.
fn runner() -> Arc<Recorded> {
    let recorded = Recorded::default();
    list(&recorded, "");
    recorded.answer("flatpak", &flatpak::remotes_args(), &recording("remotes-user.txt"), 0);
    Arc::new(recorded)
}

/// Has the runner play `lines` when flatpak runs as the user with `args`, then end with `code`.
fn play(recorded: &Recorded, args: &[String], lines: &[String], code: i32) {
    play_at(recorded, "flatpak", args, lines, code);
}

/// Has the runner play `lines` when `program` runs with `args`, then end with `code`.
fn play_at(recorded: &Recorded, program: &str, args: &[String], lines: &[String], code: i32) {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    recorded.play(program, args, &lines, ProcessOutcome::Finished { code: Some(code) });
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// The screen on an empty machine with Flatpak, `width` × `height`, in `language`, on Discover.
fn screen(recorded: &Arc<Recorded>, scratch: &Scratch, language: &str, width: u16, height: u16) -> Harness<Qpackages> {
    let settings = Settings::parse_str("settings.toml", "");
    let app = app_with(scratch, settings, recorded, with_flatpak).on_tab(Tab::Discover);
    let mut h = Harness::with_env(app, crate::test_env(), width, height);
    h.set_locale(language).set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.render();
    h
}

/// Lets the work after a confirmation run to its end and the toasts slide in.
fn settle(h: &mut Harness<Qpackages>) -> String {
    for _ in 0..4 {
        h.advance(Duration::from_millis(20));
    }
    h.screen()
}

/// Presses the dialog's own `label` button: the last one on the line of its buttons, which is the
/// line with `cancel` on it. The page under a dialog can carry a button of the same name.
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

fn flatpak_offer(id: &str) -> Offer {
    Offer { source: Source::Flatpak, package: id.to_owned() }
}

/// The calls that ran flatpak itself or asked it something, as `flatpak arg arg…` lines.
fn flatpak_calls(recorded: &Recorded) -> Vec<String> {
    recorded.command_lines().into_iter().filter(|line| line.starts_with("flatpak")).collect()
}

/// How many times the installed Flatpaks were read.
fn list_reads(recorded: &Recorded) -> usize {
    let asked = std::iter::once("flatpak".to_owned()).chain(list_args()).collect::<Vec<_>>().join(" ");
    recorded.command_lines().iter().filter(|line| **line == asked).count()
}

/// The words of each language the tests look for.
struct Words {
    cancel: &'static str,
    install_title: &'static str,
    remove_title: &'static str,
    no_permission: &'static str,
    install: &'static str,
    remove: &'static str,
    installed: &'static str,
    removed: &'static str,
    failed: &'static str,
    unused: &'static str,
}

const EN: Words = Words {
    cancel: "Cancel",
    install_title: "Install 1 Flatpak app?",
    remove_title: "Remove 1 Flatpak app?",
    no_permission: "No administrator permission is needed",
    install: "Install",
    remove: "Remove",
    installed: "Installed net.sourceforge",
    removed: "Removed net.sourceforge",
    failed: "Flatpak ended with code 1",
    unused: "runtimes no other app uses",
};

const TR: Words = Words {
    cancel: "Vazgeç",
    install_title: "1 Flatpak uygulaması kurulsun mu?",
    remove_title: "1 Flatpak uygulaması kaldırılsın mı?",
    no_permission: "Yönetici izni gerekmez",
    install: "Kur",
    remove: "Kaldır",
    installed: "net.sourceforge.ExtremeTuxRacer kuruldu",
    removed: "net.sourceforge.ExtremeTuxRacer kaldırıldı",
    failed: "Flatpak 1 koduyla bitti",
    unused: "çalışma ortamları",
};

/// Every language and a narrow and a wide screen, as the other screen tests go.
fn layouts() -> [(&'static str, &'static Words, u16, u16); 4] {
    [("en", &EN, 120, 30), ("tr", &TR, 120, 30), ("en", &EN, 60, 24), ("tr", &TR, 60, 24)]
}

/// The screen's text with the lines joined, so a sentence the narrow dialog wraps still reads
/// whole when only its words matter.
fn words(screen: &str) -> String {
    screen.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_flatpak_is_installed_for_the_user_without_the_helper_and_the_page_reads_it_again() {
    for (language, words_of, width, height) in layouts() {
        let recorded = runner();
        let mut output = lines("install-user.out");
        output.extend(lines("install-user.err"));
        play(&recorded, &flatpak::install_args(&ids(&[TUX])), &output, 0);
        let scratch = Scratch::new("flatpak-install", &[]);
        let mut h = screen(&recorded, &scratch, language, width, height);
        let reads = list_reads(&recorded);
        ask(&mut h, store::Request::Install(vec![flatpak_offer(TUX)]));
        let shown = words(&h.screen());
        for text in [words_of.install_title, TUX, words_of.no_permission] {
            assert!(shown.contains(text), "`{text}` in {language} at {width}:\n{}", h.screen());
        }
        assert!(!flatpak_calls(&recorded).iter().any(|line| line.contains(" install ")), "nothing runs before yes");
        list(&recorded, &format!("{TUX}\tuser\n"));
        confirm(&mut h, words_of.cancel, words_of.install);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(words_of.installed), "the success toast in {language}:\n{screen}");
        let install = format!("flatpak --user install --noninteractive -- flathub {TUX}");
        assert!(flatpak_calls(&recorded).contains(&install), "{:?}", recorded.command_lines());
        let call = recorded.calls().into_iter().find(|call| call.args.contains(&"install".to_owned())).expect("ran");
        assert_eq!(call.pty, None, "Flatpak prints the same lines on pipes");
        assert!(call.env.is_empty(), "its output is shown in the user's own language");
        assert_eq!(h.handoffs().len(), 0, "no password is asked");
        assert!(!h.app().transaction.has_helper(), "no helper was started");
        assert!(list_reads(&recorded) > reads, "the installed Flatpaks are read again");
        assert!(!recorded.command_lines().iter().any(|line| line.starts_with(PACMAN_PATH)), "pacman is not asked");
    }
}

#[test]
fn the_confirmation_puts_cancel_first_and_neither_enter_nor_escape_installs() {
    let recorded = runner();
    let scratch = Scratch::new("flatpak-cancel", &[]);
    let mut h = screen(&recorded, &scratch, "en", 120, 30);
    ask(&mut h, store::Request::Install(vec![flatpak_offer(TUX)]));
    let screen = h.screen();
    let buttons = screen.lines().rfind(|line| line.contains("Cancel")).expect("the dialog's buttons");
    let (cancel, install) = (buttons.find("Cancel"), buttons.rfind("Install"));
    assert!(cancel < install, "Cancel comes first:\n{screen}");
    h.press("enter");
    assert!(h.screen().contains(EN.install_title), "enter confirms nothing:\n{}", h.screen());
    h.press("esc");
    let screen = settle(&mut h);
    assert!(!screen.contains(EN.install_title), "{screen}");
    assert_eq!(flatpak_calls(&recorded).iter().filter(|line| line.contains(" install ")).count(), 0);
}

#[test]
fn a_failed_install_keeps_flatpaks_words_on_screen_and_reads_nothing_again() {
    for (language, words_of, width, height) in layouts() {
        let recorded = runner();
        let missing = "org.example.DoesNotExist";
        play(&recorded, &flatpak::install_args(&ids(&[missing])), &lines("install-missing.err"), 1);
        let scratch = Scratch::new("flatpak-failed", &[]);
        let mut h = screen(&recorded, &scratch, language, width, height);
        ask(&mut h, store::Request::Install(vec![flatpak_offer(missing)]));
        let reads = list_reads(&recorded);
        confirm(&mut h, words_of.cancel, words_of.install);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(words_of.failed), "the failure names Flatpak in {language}:\n{screen}");
        assert!(screen.contains("error: Nothing matches"), "Flatpak's own error stays in the pane:\n{screen}");
        assert!(!screen.contains("pacman"), "nothing speaks of pacman:\n{screen}");
        assert_eq!(list_reads(&recorded), reads, "nothing changed, so nothing is read again");
    }
}

#[test]
fn a_flatpak_of_the_user_is_removed_with_the_runtimes_nothing_uses_and_the_page_reads_it_again() {
    for (language, words_of, width, height) in layouts() {
        let recorded = runner();
        list(&recorded, &format!("{TUX}\tuser\n"));
        play(&recorded, &flatpak::uninstall_args(&ids(&[TUX])), &lines("uninstall-user.out"), 0);
        play(&recorded, &flatpak::uninstall_unused_args(), &lines("uninstall-unused.out"), 0);
        let scratch = Scratch::new("flatpak-remove", &[]);
        let mut h = screen(&recorded, &scratch, language, width, height);
        ask(&mut h, store::Request::Remove(vec![flatpak_offer(TUX)]));
        let shown = words(&h.screen());
        for text in [words_of.remove_title, TUX, words_of.unused, words_of.no_permission] {
            assert!(shown.contains(text), "`{text}` in {language} at {width}:\n{}", h.screen());
        }
        let reads = list_reads(&recorded);
        list(&recorded, "");
        confirm(&mut h, words_of.cancel, words_of.remove);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(words_of.removed), "the success toast in {language}:\n{screen}");
        let runs: Vec<String> =
            flatpak_calls(&recorded).into_iter().filter(|line| line.contains(" uninstall ")).collect();
        assert_eq!(
            runs,
            [
                format!("flatpak --user uninstall --noninteractive -- {TUX}"),
                "flatpak --user uninstall --unused --noninteractive".to_owned()
            ],
            "the app, then the runtimes"
        );
        assert_eq!(h.handoffs().len(), 0, "no password is asked");
        assert!(list_reads(&recorded) > reads, "the installed Flatpaks are read again");
    }
}

#[test]
fn a_flatpak_of_the_whole_system_is_removed_through_the_helper() {
    let recorded = runner();
    list(&recorded, &format!("{TUX}\tsystem\n"));
    let system = flatpak::system_uninstall_args(&ids(&[TUX]));
    play_at(&recorded, FLATPAK_PATH, &system, &lines("uninstall-system.out"), 0);
    let scratch = Scratch::new("flatpak-system", &[]);
    let mut h = screen(&recorded, &scratch, "en", 120, 30);
    ask(&mut h, store::Request::Remove(vec![flatpak_offer(TUX)]));
    let shown = words(&h.screen());
    for text in ["installed for every", "Administrator permission will be asked"] {
        assert!(shown.contains(text), "`{text}`:\n{}", h.screen());
    }
    assert!(!shown.contains(EN.unused), "the helper leaves the runtimes:\n{}", h.screen());
    confirm(&mut h, "Cancel", "Remove");
    let screen = settle(&mut h);
    assert!(screen.contains(EN.removed), "{screen}");
    assert_eq!(h.handoffs().len(), 1, "sudo asks for the password once");
    let ran: Vec<String> = recorded.command_lines().into_iter().filter(|line| line.starts_with(FLATPAK_PATH)).collect();
    assert_eq!(ran, [format!("/usr/bin/flatpak --system uninstall --noninteractive -- {TUX}")]);
    assert!(!flatpak_calls(&recorded).iter().any(|line| line.contains("--user uninstall")), "nothing of the user's");
    assert!(h.screen().contains("◆ admin"), "the helper stays up for the session:\n{}", h.screen());
}

#[test]
fn checked_cards_from_the_repositories_and_flathub_run_as_two_confirmed_steps() {
    let recorded = runner();
    recorded.answer(PACMAN, &print_install(&["gimp"]), "extra|gimp|3.2.6-1|24956108\n", 0);
    recorded.play(
        PACMAN_PATH,
        &install(&["gimp"]),
        &["(1/1) installing gimp"],
        ProcessOutcome::Finished { code: Some(0) },
    );
    play(&recorded, &flatpak::install_args(&ids(&[TUX])), &lines("install-user.out"), 0);
    let scratch = Scratch::new("flatpak-mixed", &[]);
    let mut h = screen(&recorded, &scratch, "en", 120, 30);
    let gimp = Offer { source: Source::Pacman, package: "gimp".to_owned() };
    ask(&mut h, store::Request::Install(vec![gimp, flatpak_offer(TUX)]));
    let shown = words(&h.screen());
    assert!(shown.contains("Install 1 package?"), "pacman's part first:\n{}", h.screen());
    assert!(shown.contains("1 more step follows, with its own confirmation."), "{}", h.screen());
    confirm(&mut h, "Cancel", "Install");
    let screen = settle(&mut h);
    assert!(words(&screen).contains(EN.install_title), "Flatpak's part is asked next:\n{screen}");
    assert!(!words(&screen).contains("more step"), "nothing follows it:\n{screen}");
    confirm(&mut h, "Cancel", "Install");
    let screen = settle(&mut h);
    assert!(words(&screen).contains(EN.installed), "{screen}");
    let order: Vec<String> = recorded
        .command_lines()
        .into_iter()
        .filter(|line| line.starts_with(PACMAN_PATH) || line.contains("flatpak --user install"))
        .collect();
    assert_eq!(order.len(), 2, "{order:?}");
    assert!(order[0].starts_with(PACMAN_PATH) && order[1].starts_with("flatpak"), "{order:?}");
}

#[test]
fn a_step_that_fails_stops_the_ones_after_it() {
    let recorded = runner();
    recorded.answer(PACMAN, &print_install(&["gimp"]), "extra|gimp|3.2.6-1|24956108\n", 0);
    recorded.play(PACMAN_PATH, &install(&["gimp"]), &["error: failed"], ProcessOutcome::Finished { code: Some(1) });
    let scratch = Scratch::new("flatpak-mixed-fail", &[]);
    let mut h = screen(&recorded, &scratch, "en", 120, 30);
    let gimp = Offer { source: Source::Pacman, package: "gimp".to_owned() };
    ask(&mut h, store::Request::Install(vec![gimp, flatpak_offer(TUX)]));
    confirm(&mut h, "Cancel", "Install");
    let screen = settle(&mut h);
    assert!(screen.contains("pacman ended with code 1"), "{screen}");
    assert!(!words(&screen).contains(EN.install_title), "Flatpak's part is not asked:\n{screen}");
    assert!(!flatpak_calls(&recorded).iter().any(|line| line.contains(" install ")));
}

#[test]
fn without_flathub_an_install_offers_to_add_it_first_then_goes_on() {
    let recorded = runner();
    recorded.answer("flatpak", &flatpak::remotes_args(), &recording("remotes-user-empty.txt"), 0);
    play(&recorded, &flatpak::add_flathub_args(), &[], 0);
    play(&recorded, &flatpak::install_args(&ids(&[TUX])), &lines("install-user.out"), 0);
    let scratch = Scratch::new("flatpak-no-flathub", &[]);
    let mut h = screen(&recorded, &scratch, "en", 120, 30);
    ask(&mut h, store::Request::Install(vec![flatpak_offer(TUX)]));
    let screen = settle(&mut h);
    assert!(screen.contains("Flathub is not added for"), "{screen}");
    assert!(!screen.contains(EN.install_title), "no confirmation of what cannot run:\n{screen}");
    h.click_text("Add and install");
    let shown = words(&h.screen());
    assert!(
        shown.contains("Add Flathub?") && shown.contains("https://dl.flathub.org/repo/flathub.flatpakrepo"),
        "{shown}"
    );
    assert!(shown.contains("1 more step follows"), "{shown}");
    recorded.answer("flatpak", &flatpak::remotes_args(), &recording("remotes-user.txt"), 0);
    confirm(&mut h, "Cancel", "Add Flathub");
    let screen = settle(&mut h);
    assert!(words(&screen).contains(EN.install_title), "the installation is asked next:\n{screen}");
    confirm(&mut h, "Cancel", "Install");
    assert!(words(&settle(&mut h)).contains(EN.installed));
    let runs: Vec<String> = flatpak_calls(&recorded)
        .into_iter()
        .filter(|line| line.contains("remote-add") || line.contains(" install "))
        .collect();
    assert_eq!(
        runs,
        [
            "flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo"
                .to_owned(),
            format!("flatpak --user install --noninteractive -- flathub {TUX}")
        ]
    );
}

#[test]
fn settings_say_when_flathub_is_missing_and_add_it_after_a_confirmation() {
    for (language, row, button, done, width, height) in [
        ("en", "Not added: Flatpak has nothing to install from", "Add Flathub", "Flathub was added", 120, 40),
        ("tr", "Eklenmemiş: Flatpak'ın kuracak bir yeri yok", "Flathub'ı ekle", "Flathub eklendi", 120, 40),
        ("en", "Not added", "Add Flathub", "Flathub was added", 70, 30),
        ("tr", "Eklenmemiş", "Flathub'ı ekle", "Flathub eklendi", 70, 30),
    ] {
        let recorded = runner();
        recorded.answer("flatpak", &flatpak::remotes_args(), &recording("remotes-user-empty.txt"), 0);
        play(&recorded, &flatpak::add_flathub_args(), &[], 0);
        let scratch = Scratch::new("flatpak-settings", &[]);
        let mut h = screen(&recorded, &scratch, language, width, height);
        h.send(Msg::OpenSettings);
        let screen = h.screen();
        assert!(words(&screen).contains(row), "the row in {language} at {width}:\n{screen}");
        assert!(screen.contains("Flathub"), "{screen}");
        h.send(Msg::Settings(settings_page::Msg::AddFlathub));
        let shown = words(&h.screen());
        assert!(shown.contains(button), "a confirmation first:\n{}", h.screen());
        assert!(!flatpak_calls(&recorded).iter().any(|line| line.contains("remote-add")), "nothing before yes");
        recorded.answer("flatpak", &flatpak::remotes_args(), &recording("remotes-user.txt"), 0);
        confirm(&mut h, if language == "en" { "Cancel" } else { "Vazgeç" }, button);
        let screen = settle(&mut h);
        assert!(words(&screen).contains(done), "{screen}");
        assert!(!words(&screen).contains(row), "the row goes once Flathub is there:\n{screen}");
        assert_eq!(h.handoffs().len(), 0, "no permission is asked");
    }
}

#[test]
fn settings_say_nothing_of_flathub_when_it_is_there_or_flatpak_is_off() {
    let recorded = runner();
    let scratch = Scratch::new("flatpak-settings-quiet", &[]);
    let mut h = screen(&recorded, &scratch, "en", 120, 40);
    h.send(Msg::OpenSettings);
    assert!(!h.screen().contains("Not added"), "{}", h.screen());
    recorded.answer("flatpak", &flatpak::remotes_args(), &recording("remotes-user-empty.txt"), 0);
    let settings = Settings::parse_str("settings.toml", "[sources]\nflatpak = false\n");
    let app = app_with(&scratch, settings, &recorded, with_flatpak);
    let mut off = Harness::with_env(app, crate::test_env(), 120, 40);
    off.set_locale("en").set_reduced_motion(true);
    off.send(Msg::OpenSettings);
    assert!(!off.screen().contains("Not added"), "Flatpak is off:\n{}", off.screen());
    // Without an answer about the remotes nothing is claimed.
    let unknown = Arc::new(Recorded::default());
    let mut h = screen(&unknown, &scratch, "en", 120, 40);
    h.send(Msg::OpenSettings);
    assert!(!h.screen().contains("Not added"), "{}", h.screen());
}

#[test]
fn a_card_is_marked_installed_after_its_flatpak_is_installed_and_unmarked_after_its_removal() {
    let spotify = "com.spotify.Client";
    let recorded = runner();
    play(&recorded, &flatpak::install_args(&ids(&[spotify])), &lines("install-user.out"), 0);
    play(&recorded, &flatpak::uninstall_args(&ids(&[spotify])), &lines("uninstall-user.out"), 0);
    play(&recorded, &flatpak::uninstall_unused_args(), &lines("uninstall-unused-nothing.out"), 0);
    let scratch = Scratch::new("flatpak-card", &[]);
    let mut h = screen(&recorded, &scratch, "en", 130, 30);
    let card = |h: &Harness<Qpackages>| {
        let screen = h.screen();
        let below = screen.lines().skip_while(|line| !line.contains("Spotify")).nth(2).unwrap_or_default().to_owned();
        (below, screen)
    };
    let (line, screen) = card(&h);
    assert!(!line.contains("Installed"), "not installed yet:\n{screen}");
    ask(&mut h, store::Request::Install(vec![flatpak_offer(spotify)]));
    list(&recorded, &format!("{spotify}\tuser\n"));
    confirm(&mut h, "Cancel", "Install");
    settle(&mut h);
    let (line, screen) = card(&h);
    assert!(line.contains("Installed ✓"), "the card follows the new list:\n{screen}");
    ask(&mut h, store::Request::Remove(vec![flatpak_offer(spotify)]));
    list(&recorded, "");
    confirm(&mut h, "Cancel", "Remove");
    settle(&mut h);
    let (line, screen) = card(&h);
    assert!(!line.contains("Installed"), "and again after the removal:\n{screen}");
}

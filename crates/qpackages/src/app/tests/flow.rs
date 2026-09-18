//! The transaction flow driven through the screen: nothing here reaches sudo or pacman, every
//! answer comes from the recorded runner and every handoff is recorded by the harness.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::{HandoffOutcome, ProcessOutcome};
use qpackages_core::pacman::command::{PACMAN, SUDO, print_install, print_remove, ticket_check};

use super::{app_on, env, fixture};
use crate::app::{Msg, Qpackages};
use crate::runner::Recorded;

/// What `pacman -S --print` said for gimp on the reference machine, played back for flatpak: the
/// screen shows what pacman says, whatever it is.
fn gimp_plan() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/pacman-print-install-gimp.txt");
    fs::read_to_string(path).expect("the fixture is readable")
}

/// A fresh directory under the system's temporary place for a lock file.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qpackages-flow-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a temporary directory can be created");
    dir
}

/// A runner that knows the flatpak plan and a sudo ticket in the given state.
fn runner(warm: bool) -> Arc<Recorded> {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", i32::from(!warm));
    Arc::new(recorded)
}

fn harness_with(
    recorded: &Arc<Recorded>,
    uid: Option<u32>,
    lock_dir: &Path,
    width: u16,
    height: u16,
) -> Harness<Qpackages> {
    let mut h = Harness::with_env(app_on("", recorded, uid, lock_dir), env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h
}

fn harness(recorded: &Arc<Recorded>) -> Harness<Qpackages> {
    harness_with(recorded, Some(1000), &fixture(), 120, 30)
}

/// Opens the Flatpak source, which the pretend machine lacks, and presses its install button.
/// The source is chosen by message because a narrow screen folds the sidebar away.
fn ask_to_install_flatpak(h: &mut Harness<Qpackages>) {
    h.send(Msg::Source(2));
    h.click_text(if h.screen().contains("Install flatpak") { "Install flatpak" } else { "flatpak kur" });
}

/// Clicks the last place `label` appears on screen: a dialog's action buttons sit at its bottom,
/// below a title that may carry the same word.
fn click_last(h: &mut Harness<Qpackages>, label: &str) {
    let screen = h.screen();
    let (y, line) = screen
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(label))
        .last()
        .unwrap_or_else(|| panic!("`{label}` is not on screen:\n{screen}"));
    let start = line.find(label).expect("the line contains the label");
    let x = line[..start].chars().count();
    h.click(i32::try_from(x).expect("a screen column"), i32::try_from(y).expect("a screen row"));
}

/// Toasts slide in; a moment passes so the words are on screen.
fn settle(h: &mut Harness<Qpackages>) -> String {
    h.advance(Duration::from_millis(50));
    h.screen()
}

#[test]
fn the_confirmation_says_exactly_what_pacman_printed() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 1);
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    let screen = h.screen();
    for text in [
        "Install 9 packages?",
        "extra/babl  0.1.128-1",
        "1.5 MiB",
        "extra/gimp  3.2.6-1",
        "23.8 MiB",
        "Total download 39.9 MiB",
        "Your password will be asked",
        "Cancel",
        "Install",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert_eq!(
        recorded.command_lines(),
        ["pacman -S --print --print-format %r|%n|%v|%s flatpak", "sudo -n true"],
        "planning runs the print and the ticket check and nothing else"
    );
    assert_eq!(recorded.calls()[0].env, [("LC_ALL".to_owned(), "C".to_owned()), ("LANG".to_owned(), "C".to_owned())]);
    assert!(recorded.calls()[1].env.is_empty(), "the ticket check runs in the user's environment");
    assert!(h.handoffs().is_empty(), "nothing is authorized before the user applies");
}

#[test]
fn escape_closes_the_confirmation_and_nothing_runs() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 1);
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    assert!(h.screen().contains("Install 9 packages?"));
    h.press("esc");
    let screen = h.screen();
    assert!(!screen.contains("Install 9 packages?"), "{screen}");
    assert!(screen.contains("Flatpak is not installed"), "the source screen is back:\n{screen}");
    assert_eq!(recorded.calls().len(), 2, "only the planning calls were made");
    assert!(h.handoffs().is_empty());
}

#[test]
fn a_cold_ticket_is_warmed_on_the_real_terminal_before_pacman_runs() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 1);
    recorded.play(
        SUDO,
        &["pacman", "-S", "--noconfirm", "--needed", "flatpak"],
        [":: Retrieving packages...", "(1/1) installing flatpak"].as_slice(),
        ProcessOutcome::Finished { code: Some(0) },
    );
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    assert_eq!(h.handoffs().len(), 1, "exactly one handoff");
    assert_eq!(h.handoffs()[0].program, "sudo");
    assert_eq!(h.handoffs()[0].args, ["-v"]);
    assert!(h.handoffs()[0].notice.as_deref().is_some_and(|notice| notice.contains("password")));
    let lines = recorded.command_lines();
    assert_eq!(lines.last().map(String::as_str), Some("sudo pacman -S --noconfirm --needed flatpak"));
    let run = recorded.calls().last().cloned().expect("pacman ran");
    assert_eq!(run.pty, Some((79, 9)), "pacman runs on a pseudo-terminal the size of the output pane at 120x30");
    assert!(run.env.is_empty(), "the shown output keeps the user's language");
    let screen = settle(&mut h);
    assert!(screen.contains("Installed flatpak"), "the success toast:\n{screen}");
    assert!(!screen.contains("Hold to stop"), "the pane closes after a success:\n{screen}");
    assert!(screen.contains("Flatpak is not installed"), "the source screen is still there:\n{screen}");
}

/// A runner that plays a successful flatpak install with a warm ticket, so transactions can be
/// run one after another without a handoff.
fn installing_runner() -> Arc<Recorded> {
    let recorded = runner(true);
    recorded.play(
        SUDO,
        &["pacman", "-S", "--noconfirm", "--needed", "flatpak"],
        ["done"].as_slice(),
        ProcessOutcome::Finished { code: Some(0) },
    );
    recorded
}

/// The pseudo-terminal sizes of every stream the runner was asked for, oldest first.
fn pty_sizes(recorded: &Recorded) -> Vec<(u16, u16)> {
    recorded.calls().iter().filter_map(|call| call.pty).collect()
}

#[test]
fn pacman_gets_a_terminal_as_wide_as_the_pane_and_follows_a_resize() {
    let recorded = installing_runner();
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    assert_eq!(pty_sizes(&recorded), [(79, 9)], "at 120x30 the pane shows 79 columns of 9 lines");
    h.resize(60, 20);
    assert_eq!(h.app().size, qframe::prelude::Size::new(60, 20), "the application hears the new size");
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    assert_eq!(pty_sizes(&recorded), [(79, 9), (47, 9)], "the next transaction gets the smaller pane");
}

#[test]
fn a_tiny_screen_still_gives_pacman_a_usable_terminal() {
    let recorded = installing_runner();
    let mut h = harness_with(&recorded, Some(1000), &fixture(), 30, 8);
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    assert_eq!(pty_sizes(&recorded), [(20, 3)], "never narrower than pacman's bars can be read");
    assert!(settle(&mut h).contains("Installed"), "the toast fits what it can:\n{}", h.screen());
}

#[test]
fn a_warm_ticket_asks_for_no_handoff() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 0);
    recorded.play(
        SUDO,
        &["pacman", "-S", "--noconfirm", "--needed", "flatpak"],
        ["done"].as_slice(),
        ProcessOutcome::Finished { code: Some(0) },
    );
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    assert!(h.screen().contains("Authorization is ready"), "{}", h.screen());
    click_last(&mut h, "Install");
    assert!(h.handoffs().is_empty(), "a warm ticket needs no handoff");
    assert_eq!(
        recorded.command_lines().last().map(String::as_str),
        Some("sudo pacman -S --noconfirm --needed flatpak")
    );
    assert!(settle(&mut h).contains("Installed flatpak"));
}

#[test]
fn a_refused_authorization_runs_nothing() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 1);
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    h.set_handoff_outcome(HandoffOutcome::Finished { code: Some(1) });
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    assert_eq!(h.handoffs().len(), 1);
    assert_eq!(recorded.calls().len(), 2, "no stream was started:\n{:?}", recorded.command_lines());
    let screen = settle(&mut h);
    assert!(screen.contains("Not authorized"), "{screen}");
    assert!(!screen.contains("Install 9 packages?"), "the dialog is gone:\n{screen}");
    assert!(screen.contains("Flatpak is not installed"), "the list is back:\n{screen}");
}

#[test]
fn a_failed_run_keeps_its_output_on_screen_without_escape_sequences() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 0);
    let lines = [
        "\u{1b}]3008;start=2026-09-18T01:00:00Z\u{7}\u{1b}[?25l:: Retrieving packages...",
        "\u{1b}[1;31merror:\u{1b}[0m failed retrieving file 'flatpak-1.16.0-1-x86_64.pkg.tar.zst'",
    ];
    recorded.play(
        SUDO,
        &["pacman", "-S", "--noconfirm", "--needed", "flatpak"],
        lines.as_slice(),
        ProcessOutcome::Finished { code: Some(1) },
    );
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    let screen = settle(&mut h);
    for text in [
        "pacman ended with code 1",
        "Installing flatpak did not go through",
        ":: Retrieving packages...",
        "error: failed retrieving file",
        "Close",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains('\u{1b}'), "escape sequences are filtered:\n{screen}");
    assert!(!screen.contains("3008"), "the OSC payload is gone:\n{screen}");
    assert!(screen.contains("Flatpak is not installed"), "the source screen stays above the pane:\n{screen}");
    h.click_text("Close");
    assert!(!h.screen().contains("Retrieving packages"), "the pane closes:\n{}", h.screen());
}

#[test]
fn the_checked_packages_are_removed_with_their_own_dependencies() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_remove(&["bash"]), "bash|5.3.15-1\n", 0);
    recorded.answer(SUDO, &ticket_check(), "", 0);
    recorded.play(
        SUDO,
        &["pacman", "-Rs", "--noconfirm", "bash"],
        ["(1/1) removing bash"].as_slice(),
        ProcessOutcome::Finished { code: Some(0) },
    );
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    assert!(!h.screen().contains("Remove"), "nothing to remove before a check:\n{}", h.screen());
    h.send(Msg::Toggle(0));
    let screen = h.screen();
    assert!(screen.contains("1 package checked"), "{screen}");
    assert!(screen.contains("Remove"), "{screen}");
    h.press("delete");
    let screen = h.screen();
    for text in ["Remove 1 package?", "bash  5.3.15-1", "Authorization is ready"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Total download"), "a removal downloads nothing:\n{screen}");
    click_last(&mut h, "Remove");
    assert_eq!(recorded.command_lines().last().map(String::as_str), Some("sudo pacman -Rs --noconfirm bash"));
    assert!(settle(&mut h).contains("Removed bash"));
}

#[test]
fn a_plan_pacman_refuses_is_shown_and_nothing_else_happens() {
    let recorded = Recorded::default();
    recorded.fail(PACMAN, &print_install(&["flatpak"]), "error: target not found: flatpak\n", 1);
    recorded.answer(SUDO, &ticket_check(), "", 0);
    let recorded = Arc::new(recorded);
    let mut h = harness(&recorded);
    ask_to_install_flatpak(&mut h);
    let screen = settle(&mut h);
    assert!(screen.contains("pacman could not plan this"), "{screen}");
    assert!(screen.contains("target not found: flatpak"), "{screen}");
    assert!(!screen.contains("packages?"), "{screen}");
    assert!(h.handoffs().is_empty());
}

#[test]
fn a_held_lock_shows_the_notice_instead_of_the_confirmation() {
    let dir = scratch("held");
    fs::write(dir.join("db.lck"), "").expect("the lock can be written");
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    recorded.answer(SUDO, &ticket_check(), "", 1);
    let recorded = Arc::new(recorded);
    let mut h = harness_with(&recorded, Some(1000), &dir, 120, 30);
    ask_to_install_flatpak(&mut h);
    let screen = h.screen();
    for text in ["The package database is locked", "Another transaction is running", "Since 20", "locks its database"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Install 9 packages?"), "no confirmation:\n{screen}");
    assert!(!screen.contains("Total download"), "{screen}");
    assert!(!screen.to_lowercase().contains("remove lock"), "{screen}");
    h.click_text("Close");
    assert!(!h.screen().contains("database is locked"), "{}", h.screen());
    assert!(h.handoffs().is_empty());
    assert_eq!(recorded.calls().len(), 2, "planning ran, nothing else");
    assert!(dir.join("db.lck").exists(), "the lock is never removed");
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn running_as_root_is_said_in_the_header_and_makes_the_aur_unusable() {
    let mut h = harness_with(&runner(true), Some(0), &fixture(), 120, 30);
    let screen = h.screen();
    assert!(screen.contains("Running as root"), "{screen}");
    assert!(screen.contains("not as root"), "{screen}");
    let (px, py) = h.find("Pacman").expect("pacman is listed");
    let (ax, ay) = h.find("AUR").expect("the AUR is listed");
    let pacman = h.fg(u16::try_from(px).unwrap(), u16::try_from(py).unwrap());
    let aur = h.fg(u16::try_from(ax).unwrap(), u16::try_from(ay).unwrap());
    assert_ne!(pacman, aur, "the AUR row is faint");
    h.click_text("AUR");
    let screen = h.screen();
    assert!(screen.contains("AUR cannot be used as root"), "{screen}");
    let mut ordinary = harness_with(&runner(true), Some(1000), &fixture(), 120, 30);
    assert!(!ordinary.screen().contains("Running as root"));
    assert!(!ordinary.screen().contains("not as root"));
    ordinary.click_text("AUR");
    assert!(ordinary.screen().contains("AUR comes in a later version"), "{}", ordinary.screen());
}

#[test]
fn the_dialog_keeps_the_rules_in_ascii_and_on_a_narrow_screen() {
    for (width, height) in [(60, 20), (80, 24), (120, 30)] {
        let recorded = Recorded::default();
        recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
        recorded.answer(SUDO, &ticket_check(), "", 1);
        let recorded = Arc::new(recorded);
        let mut h = harness_with(&recorded, Some(1000), &fixture(), width, height);
        h.set_glyph_mode(GlyphMode::Ascii);
        ask_to_install_flatpak(&mut h);
        let screen = h.screen();
        assert!(screen.contains("packages?"), "{width}x{height}:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
        }
        h.set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("kurulsun mu?"), "{width}x{height}:\n{screen}");
        assert!(!screen.contains('⟦'), "a key is missing in Turkish:\n{screen}");
    }
}

/// With `QUVYTA_REVIEW=1`, writes the confirmation, the lock notice and a failed run's pane in
/// both languages to `target/qpackages-flow-review.html` in colour, and prints them.
#[test]
fn visual_review_flow() {
    if std::env::var_os("QUVYTA_REVIEW").is_none() {
        return;
    }
    let mut fragments = Vec::new();
    for locale in ["en", "tr"] {
        let recorded = Recorded::default();
        recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
        recorded.answer(SUDO, &ticket_check(), "", 1);
        let lines =
            [":: Retrieving packages...", "error: failed retrieving file 'flatpak-1.16.0-1-x86_64.pkg.tar.zst'"];
        let args = ["pacman", "-S", "--noconfirm", "--needed", "flatpak"];
        recorded.play(SUDO, &args, lines.as_slice(), ProcessOutcome::Finished { code: Some(1) });
        let recorded = Arc::new(recorded);
        let mut h = harness(&recorded);
        h.set_locale(locale);
        ask_to_install_flatpak(&mut h);
        fragments.push(h.html(&format!("confirmation {locale}")));
        println!("confirmation {locale}\n{}", h.screen());
        click_last(&mut h, if locale == "en" { "Install" } else { "Kur" });
        let screen = settle(&mut h);
        fragments.push(h.html(&format!("failed run {locale}")));
        println!("failed run {locale}\n{screen}");

        let dir = scratch(&format!("review-{locale}"));
        fs::write(dir.join("db.lck"), "").expect("the lock can be written");
        let mut h = harness_with(&recorded, Some(1000), &dir, 120, 30);
        h.set_locale(locale);
        ask_to_install_flatpak(&mut h);
        fragments.push(h.html(&format!("lock {locale}")));
        println!("lock {locale}\n{}", h.screen());
        fs::remove_dir_all(dir).expect("cleanup");
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/qpackages-flow-review.html");
    fs::write(path, qframe::runtime::html_page(&fragments)).expect("review page written");
}

//! The transaction flow driven through the screen: nothing here reaches sudo or pacman. Planning
//! answers come from the recorded runner, every handoff is recorded by the harness, and the
//! helper is the real root-side loop on a thread, playing pacman from the same recording.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::{HandoffOutcome, ProcessOutcome};
use qpackages_core::helper::PACMAN_PATH;
use qpackages_core::pacman::command::{PACMAN, install, print_install, print_remove, remove};

use super::{after_reads, app_on, env, fixture};
use crate::app::{Msg, Qpackages};
use crate::helper::session::InProcess;
use crate::runner::Recorded;
use crate::{installed, settings_page};

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

/// A runner that knows the flatpak plan.
pub(super) fn runner() -> Arc<Recorded> {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_install(&["flatpak"]), &gimp_plan(), 0);
    Arc::new(recorded)
}

/// A runner that knows the flatpak plan and plays its installation with `lines`, ending `outcome`.
fn installing(lines: &[&str], outcome: ProcessOutcome) -> Arc<Recorded> {
    let recorded = runner();
    recorded.play(PACMAN_PATH, &install(&["flatpak"]), lines, outcome);
    recorded
}

/// A runner whose flatpak installation succeeds.
fn installing_runner() -> Arc<Recorded> {
    installing(&["done"], ProcessOutcome::Finished { code: Some(0) })
}

/// The screen with helpers that run as `helper_uid`, which is root unless a test says otherwise.
struct Screen {
    h: Harness<Qpackages>,
    helper: Arc<InProcess>,
}

fn screen_with(recorded: &Arc<Recorded>, helper_uid: u32, lock_dir: &Path, width: u16, height: u16) -> Screen {
    let helper = InProcess::new(recorded, helper_uid);
    let mut h = Harness::with_env(app_on("", recorded, &helper, Some(1000), lock_dir), env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    Screen { h, helper }
}

fn screen(recorded: &Arc<Recorded>) -> Screen {
    screen_with(recorded, 0, &fixture(), 120, 30)
}

/// Opens the settings page, where Flatpak, which the pretend machine lacks, is on offer, and
/// presses its install button. The page is opened by message so a narrow screen needs no aim.
pub(super) fn ask_to_install_flatpak(h: &mut Harness<Qpackages>) {
    h.send(Msg::OpenSettings);
    let screen = h.screen();
    match ["Install flatpak", "flatpak kur"].into_iter().find(|label| screen.contains(label)) {
        Some(label) => {
            h.click_text(label);
        }
        // A screen too small for the whole label presses it by its message.
        None => {
            h.send(Msg::Settings(settings_page::Msg::Install(qpackages_core::sources::Source::Flatpak)));
        }
    }
}

/// Where `label` stands as a whole word in `line`: `Install` in a button, not in `Installed`.
fn word_at(line: &str, label: &str) -> Option<usize> {
    line.match_indices(label)
        .map(|(start, _)| start)
        .find(|&start| !line[start + label.len()..].starts_with(char::is_alphabetic))
}

/// Clicks the last place `label` appears on screen as a word: a dialog's action buttons sit at
/// its bottom, below a title that may carry the same word; a toast's `Installed` is not `Install`.
pub(super) fn click_last(h: &mut Harness<Qpackages>, label: &str) {
    let screen = h.screen();
    let (y, line, start) = screen
        .lines()
        .enumerate()
        .filter_map(|(y, line)| word_at(line, label).map(|start| (y, line, start)))
        .last()
        .unwrap_or_else(|| panic!("`{label}` is not on screen:\n{screen}"));
    let x = line[..start].chars().count();
    h.click(i32::try_from(x).expect("a screen column"), i32::try_from(y).expect("a screen row"));
}

/// Lets the chain after a confirmation run to its end: the handoff, the helper's start in the
/// background, then the transaction. Toasts slide in; a moment passes so the words are on screen.
pub(super) fn settle(h: &mut Harness<Qpackages>) -> String {
    for _ in 0..4 {
        h.advance(Duration::from_millis(20));
    }
    h.screen()
}

/// Confirms the flatpak installation and lets it run through.
pub(super) fn install_flatpak(h: &mut Harness<Qpackages>) -> String {
    ask_to_install_flatpak(h);
    click_last(h, "Install");
    settle(h)
}

/// The pacman runs the helpers were asked for, as `program arg arg…` lines.
fn pacman_runs(recorded: &Recorded) -> Vec<String> {
    recorded.command_lines().into_iter().filter(|line| line.starts_with(PACMAN_PATH)).collect()
}

#[test]
fn the_confirmation_says_exactly_what_pacman_printed() {
    let recorded = runner();
    let Screen { mut h, helper } = screen(&recorded);
    ask_to_install_flatpak(&mut h);
    let screen = h.screen();
    for text in [
        "Install 9 packages?",
        "extra/babl  0.1.128-1",
        "1.5 MiB",
        "extra/gimp  3.2.6-1",
        "23.8 MiB",
        "Total download 39.9 MiB",
        "Administrator permission will be asked;",
        "Cancel",
        "Install",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    let calls = after_reads(&recorded);
    let lines: Vec<String> = calls.iter().map(|call| format!("{} {}", call.program, call.args.join(" "))).collect();
    assert_eq!(lines, ["pacman -S --print --print-format %r|%n|%v|%s -- flatpak"], "planning runs the print alone");
    assert_eq!(calls[0].env, [("LC_ALL".to_owned(), "C".to_owned()), ("LANG".to_owned(), "C".to_owned())]);
    assert!(h.handoffs().is_empty(), "nothing is authorized before the user applies");
    assert_eq!(helper.starts(), 0, "no helper before the user applies");
    assert!(!screen.contains("admin"), "no badge without a helper:\n{screen}");
}

#[test]
fn escape_closes_the_confirmation_and_nothing_runs() {
    let recorded = runner();
    let Screen { mut h, helper } = screen(&recorded);
    ask_to_install_flatpak(&mut h);
    assert!(h.screen().contains("Install 9 packages?"));
    h.press("esc");
    let screen = h.screen();
    assert!(!screen.contains("Install 9 packages?"), "{screen}");
    assert!(screen.contains("Install flatpak"), "the settings page is back:\n{screen}");
    assert_eq!(after_reads(&recorded).len(), 1, "only the planning call was made");
    assert!(h.handoffs().is_empty());
    assert_eq!(helper.starts(), 0);
}

#[test]
fn the_first_transaction_asks_once_and_starts_the_helper_that_runs_pacman() {
    let recorded = installing(
        &[":: Retrieving packages...", "(1/1) installing flatpak"],
        ProcessOutcome::Finished { code: Some(0) },
    );
    let Screen { mut h, helper } = screen(&recorded);
    let screen = install_flatpak(&mut h);
    assert_eq!(h.handoffs().len(), 1, "exactly one handoff");
    assert_eq!(h.handoffs()[0].program, "sudo");
    assert_eq!(h.handoffs()[0].args, ["-v"]);
    assert!(h.handoffs()[0].notice.as_deref().is_some_and(|notice| notice.contains("password")));
    assert_eq!(helper.starts(), 1, "the helper started after the handoff");
    assert_eq!(pacman_runs(&recorded), ["/usr/bin/pacman -S --needed --noconfirm -- flatpak"]);
    let run = recorded.calls().into_iter().rev().find(|call| call.program == PACMAN_PATH).expect("pacman ran");
    assert_eq!(run.pty, Some((107, 9)), "pacman runs on a pseudo-terminal the size of the output pane at 120x30");
    assert_eq!(run.env[0], ("PATH".to_owned(), "/usr/bin:/usr/sbin".to_owned()), "the helper sets the path");
    assert!(screen.contains("Installed flatpak"), "the success toast:\n{screen}");
    assert!(!screen.contains("Hold to stop"), "the pane closes after a success:\n{screen}");
    assert!(screen.contains("Install flatpak"), "the source screen is still there:\n{screen}");
    assert!(screen.contains("◆ admin"), "the header says the helper is up:\n{screen}");
}

#[test]
fn later_transactions_go_straight_to_the_helper() {
    let recorded = installing_runner();
    let Screen { mut h, helper } = screen(&recorded);
    install_flatpak(&mut h);
    ask_to_install_flatpak(&mut h);
    let screen = h.screen();
    assert!(screen.contains("Administrator permission was given for this"), "{screen}");
    click_last(&mut h, "Install");
    assert!(settle(&mut h).contains("Installed flatpak"));
    assert_eq!(h.handoffs().len(), 1, "no second handoff");
    assert_eq!(helper.starts(), 1, "the same helper");
    assert_eq!(pacman_runs(&recorded).len(), 2);
}

#[test]
fn a_helper_that_died_is_replaced_by_the_next_transaction() {
    let recorded = installing_runner();
    let Screen { mut h, helper } = screen(&recorded);
    install_flatpak(&mut h);
    helper.kill();
    ask_to_install_flatpak(&mut h);
    let screen = h.screen();
    assert!(screen.contains("Administrator permission will be asked"), "{screen}");
    assert!(!screen.contains("admin\n") && !screen.contains("◆ admin"), "the badge went with the helper:\n{screen}");
    click_last(&mut h, "Install");
    assert!(settle(&mut h).contains("Installed flatpak"));
    assert_eq!(h.handoffs().len(), 2, "sudo asks again");
    assert_eq!(helper.starts(), 2, "a new helper");
    assert_eq!(pacman_runs(&recorded).len(), 2);
}

#[test]
fn clicking_the_badge_lets_the_helper_go() {
    let recorded = installing_runner();
    let Screen { mut h, helper } = screen(&recorded);
    install_flatpak(&mut h);
    assert!(h.app().transaction.has_helper());
    let (x, y) = h.find("◆ admin").expect("the badge is on screen");
    h.click(x + 2, y);
    assert!(!h.app().transaction.has_helper(), "the helper is gone");
    assert!(!h.screen().contains("◆ admin"), "{}", h.screen());
    install_flatpak(&mut h);
    assert_eq!(h.handoffs().len(), 2, "the next transaction asks again");
    assert_eq!(helper.starts(), 2);
}

#[test]
fn the_badge_is_drawn_in_every_glyph_mode_and_both_languages() {
    let recorded = installing_runner();
    let Screen { mut h, .. } = screen(&recorded);
    install_flatpak(&mut h);
    h.set_glyph_mode(GlyphMode::Ascii);
    assert!(h.screen().contains("* admin"), "{}", h.screen());
    h.set_glyph_mode(GlyphMode::Nerd);
    assert!(h.screen().contains("\u{f033e} admin"), "{}", h.screen());
    h.set_locale("tr");
    assert!(h.screen().contains("yönetici"), "{}", h.screen());
}

#[test]
fn pacman_gets_a_terminal_as_wide_as_the_pane_and_follows_a_resize() {
    let recorded = installing_runner();
    let Screen { mut h, .. } = screen(&recorded);
    install_flatpak(&mut h);
    assert_eq!(pty_sizes(&recorded), [(107, 9)], "at 120x30 the pane shows 107 columns of 9 lines");
    h.resize(60, 20);
    assert_eq!(h.app().size, qframe::prelude::Size::new(60, 20), "the application hears the new size");
    install_flatpak(&mut h);
    assert_eq!(pty_sizes(&recorded), [(107, 9), (47, 9)], "the next transaction gets the smaller pane");
}

/// The pseudo-terminal sizes of every stream the runner was asked for, oldest first.
fn pty_sizes(recorded: &Recorded) -> Vec<(u16, u16)> {
    recorded.calls().iter().filter_map(|call| call.pty).collect()
}

#[test]
fn a_tiny_screen_still_gives_pacman_a_usable_terminal() {
    let recorded = installing_runner();
    let Screen { mut h, .. } = screen_with(&recorded, 0, &fixture(), 30, 8);
    let screen = install_flatpak(&mut h);
    assert_eq!(pty_sizes(&recorded), [(20, 3)], "never narrower than pacman's bars can be read");
    assert!(screen.contains("Installed"), "the toast fits what it can:\n{screen}");
}

#[test]
fn a_refused_authorization_runs_nothing() {
    let recorded = installing_runner();
    let Screen { mut h, helper } = screen(&recorded);
    h.set_handoff_outcome(HandoffOutcome::Finished { code: Some(1) });
    let screen = install_flatpak(&mut h);
    assert_eq!(h.handoffs().len(), 1);
    assert_eq!(helper.starts(), 0, "no helper was started");
    assert!(pacman_runs(&recorded).is_empty(), "{:?}", recorded.command_lines());
    assert!(screen.contains("Permission was not given"), "{screen}");
    assert!(screen.contains("Nothing was changed."), "{screen}");
    assert!(!screen.contains("Install 9 packages?"), "the dialog is gone:\n{screen}");
    assert!(screen.contains("Install flatpak"), "the list is back:\n{screen}");
}

#[test]
fn a_helper_that_refuses_to_start_runs_nothing_and_says_why() {
    let recorded = installing_runner();
    let Screen { mut h, helper } = screen_with(&recorded, 1000, &fixture(), 120, 30);
    let screen = install_flatpak(&mut h);
    assert_eq!(helper.starts(), 1);
    assert!(pacman_runs(&recorded).is_empty(), "{:?}", recorded.command_lines());
    assert!(screen.contains("Permission was not given"), "{screen}");
    assert!(screen.contains("not running as the administrator"), "{screen}");
    assert!(!h.app().transaction.has_helper());
    assert!(!screen.contains("◆ admin"), "{screen}");
}

#[test]
fn a_failed_run_keeps_its_output_on_screen_without_escape_sequences() {
    let lines = [
        "\u{1b}]3008;start=2026-09-18T01:00:00Z\u{7}\u{1b}[?25l:: Retrieving packages...",
        "\u{1b}[1;31merror:\u{1b}[0m failed retrieving file 'flatpak-1.16.0-1-x86_64.pkg.tar.zst'",
    ];
    let recorded = installing(&lines, ProcessOutcome::Finished { code: Some(1) });
    let Screen { mut h, .. } = screen(&recorded);
    let screen = install_flatpak(&mut h);
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
    assert!(screen.contains("Install flatpak"), "the source screen stays above the pane:\n{screen}");
    assert!(h.app().transaction.has_helper(), "a failed pacman leaves the helper up");
    h.click_text("Close");
    assert!(!h.screen().contains("Retrieving packages"), "the pane closes:\n{}", h.screen());
}

#[test]
fn the_checked_packages_are_removed_with_their_own_dependencies_and_settings() {
    let recorded = Recorded::default();
    recorded.answer(PACMAN, &print_remove(&["bash"]), "bash|5.3.15-1\n", 0);
    recorded.play(
        PACMAN_PATH,
        &remove(&["bash"]),
        &["(1/1) removing bash"],
        ProcessOutcome::Finished { code: Some(0) },
    );
    let recorded = Arc::new(recorded);
    let Screen { mut h, .. } = screen(&recorded);
    h.send(Msg::Tab(crate::app::Tab::Installed.index()));
    h.send(Msg::Installed(installed::Msg::Show(1)));
    assert!(!h.screen().contains("Remove"), "nothing to remove before a check:\n{}", h.screen());
    h.send(Msg::Installed(installed::Msg::Toggle(0)));
    let screen = h.screen();
    assert!(screen.contains("1 package checked"), "{screen}");
    assert!(screen.contains("Remove"), "{screen}");
    h.press("delete");
    let screen = h.screen();
    for text in ["Remove 1 package?", "bash  5.3.15-1", "Their settings files are deleted too.", "will be asked"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Total download"), "a removal downloads nothing:\n{screen}");
    // The dialog's own button, beside Cancel: the summary line under the dialog says Remove too.
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Cancel") && line.contains("Remove"))
        .expect("the dialog's buttons");
    let x = line[..line.rfind("Remove").expect("the button")].chars().count();
    h.click(i32::try_from(x).expect("a column"), i32::try_from(y).expect("a row"));
    let screen = settle(&mut h);
    assert_eq!(pacman_runs(&recorded), ["/usr/bin/pacman -Rns --noconfirm -- bash"]);
    assert!(screen.contains("Removed bash"), "{screen}");
}

#[test]
fn a_plan_pacman_refuses_is_shown_and_nothing_else_happens() {
    let recorded = Recorded::default();
    recorded.fail(PACMAN, &print_install(&["flatpak"]), "error: target not found: flatpak\n", 1);
    let recorded = Arc::new(recorded);
    let Screen { mut h, .. } = screen(&recorded);
    ask_to_install_flatpak(&mut h);
    h.advance(Duration::from_millis(50));
    let screen = h.screen();
    assert!(screen.contains("pacman could not plan this"), "{screen}");
    assert!(screen.contains("target not found: flatpak"), "{screen}");
    assert!(!screen.contains("packages?"), "{screen}");
    assert!(h.handoffs().is_empty());
}

#[test]
fn a_held_lock_shows_the_notice_instead_of_the_confirmation() {
    let dir = scratch("held");
    fs::write(dir.join("db.lck"), "").expect("the lock can be written");
    let recorded = runner();
    let Screen { mut h, .. } = screen_with(&recorded, 0, &dir, 120, 30);
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
    assert_eq!(after_reads(&recorded).len(), 1, "planning ran, nothing else");
    assert!(dir.join("db.lck").exists(), "the lock is never removed");
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn the_dialog_keeps_the_rules_in_ascii_and_on_a_narrow_screen() {
    for (width, height) in [(60, 20), (80, 24), (120, 30)] {
        let recorded = installing_runner();
        let Screen { mut h, .. } = screen_with(&recorded, 0, &fixture(), width, height);
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
        click_last(&mut h, "Kur");
        let screen = settle(&mut h);
        assert!(screen.contains("* yönetici"), "the badge at {width}x{height}:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|', '(', ')'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
        }
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
        let lines =
            [":: Retrieving packages...", "error: failed retrieving file 'flatpak-1.16.0-1-x86_64.pkg.tar.zst'"];
        let recorded = installing(&lines, ProcessOutcome::Finished { code: Some(1) });
        let Screen { mut h, .. } = screen(&recorded);
        h.set_locale(locale);
        ask_to_install_flatpak(&mut h);
        fragments.push(h.html(&format!("confirmation {locale}")));
        println!("confirmation {locale}\n{}", h.screen());
        click_last(&mut h, if locale == "en" { "Install" } else { "Kur" });
        let screen = settle(&mut h);
        fragments.push(h.html(&format!("failed run {locale}")));
        println!("failed run {locale}\n{screen}");
        ask_to_install_flatpak(&mut h);
        fragments.push(h.html(&format!("confirmation with the helper up {locale}")));
        println!("confirmation with the helper up {locale}\n{}", h.screen());

        let dir = scratch(&format!("review-{locale}"));
        fs::write(dir.join("db.lck"), "").expect("the lock can be written");
        let Screen { mut h, .. } = screen_with(&recorded, 0, &dir, 120, 30);
        h.set_locale(locale);
        ask_to_install_flatpak(&mut h);
        fragments.push(h.html(&format!("lock {locale}")));
        println!("lock {locale}\n{}", h.screen());
        fs::remove_dir_all(dir).expect("cleanup");
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/qpackages-flow-review.html");
    fs::write(path, qframe::runtime::html_page(&fragments)).expect("review page written");
}

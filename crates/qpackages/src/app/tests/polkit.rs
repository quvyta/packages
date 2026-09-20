//! The transaction flow where polkit asks for permission: pkexec is never run. The harness records
//! the detached handoff and answers it with a stand-in child, which the tests play as the helper.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::{DetachedOutcome, LiveChild, TestChild};
use qframe::storage::Settings;

use super::flow::{ask_to_install_flatpak, click_last, install_flatpak, runner, settle};
use super::{after_reads, env, fixture, nowhere};
use crate::app::{Machine, Qpackages};
use crate::helper::pkexec;
use crate::helper::session::{InProcess, locale};
use crate::runner::Recorded;
use crate::settings;

/// A pretend machine with pacman, paru and polkit's pkexec.
fn with_polkit(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot", "pkexec"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// The screen on the machine with polkit, following `settings`. The sudo path's helpers are
/// in-process ones, so a test that ends up on sudo never reaches the real thing either.
fn screen(settings: &str) -> (Harness<Qpackages>, Arc<InProcess>, Arc<Recorded>) {
    let recorded = runner();
    let helper = InProcess::new(&recorded, 0);
    let settings = Settings::parse_str("settings.toml", settings).schema(settings::schema());
    let machine = Machine {
        dbpath: &fixture(),
        sync_dir: &nowhere(),
        applications: &nowhere(),
        check_dir: None,
        lock_dir: &fixture(),
        lookup: Arc::new(with_polkit),
        runner: Arc::clone(&recorded) as Arc<dyn crate::runner::Runner>,
        helper: helper.start_fn(),
        uid: Some(1000),
        utc_offset: 0,
        app_catalog: &nowhere(),
        flatpak_catalogs: &[],
        appearance: crate::testing::appearance_apart(),
    };
    let mut h = Harness::with_env(Qpackages::new(machine, &settings), env(), 120, 30);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    (h, helper, recorded)
}

/// The screen with pkexec answering by starting a helper the test plays.
fn granted() -> (Harness<Qpackages>, TestChild) {
    let (mut h, _, _) = screen("");
    let (child, program) = LiveChild::for_tests();
    h.set_detached_outcome(DetachedOutcome::Detached { child, first_line: "ready 1".to_owned() });
    (h, program)
}

/// Steps the harness until `done` holds, for at most a couple of seconds: the helper's answers
/// cross a background task and the application's messages before they are on screen.
fn until(h: &mut Harness<Qpackages>, what: &str, done: impl Fn(&mut Harness<Qpackages>) -> bool) {
    for _ in 0..200 {
        if done(h) {
            return;
        }
        h.advance(Duration::from_millis(10));
    }
    panic!("{what} did not happen:\n{}", h.screen());
}

/// Plays the helper through its `nth` installation of flatpak: waits for that request, says a
/// line of pacman's output and that it ended with `code`.
fn serve_install(h: &mut Harness<Qpackages>, program: &TestChild, nth: usize, code: i32) {
    let installs = |program: &TestChild| program.written().iter().filter(|line| *line == "install flatpak").count();
    until(h, "the install request", |_| installs(program) >= nth);
    program.say("line (1/1) installing flatpak");
    program.say(format!("done {code}"));
}

#[test]
fn auto_asks_polkit_where_pkexec_is_found() {
    let (mut h, helper, _) = screen("");
    h.set_detached_outcome(DetachedOutcome::Finished { code: Some(126) });
    install_flatpak(&mut h);
    assert!(h.handoffs().is_empty(), "sudo is not asked");
    assert_eq!(helper.starts(), 0, "the sudo path's helper is not started");
    assert_eq!(h.detached_handoffs().len(), 1);
    let request = &h.detached_handoffs()[0];
    assert_eq!(request.program, "/usr/bin/pkexec");
    let exe = std::env::current_exe().expect("the test knows its own path");
    assert!(exe.is_absolute());
    let expected: Vec<std::ffi::OsString> =
        pkexec::args(&exe, locale().as_deref()).into_iter().map(Into::into).collect();
    assert_eq!(request.args, expected);
    assert_eq!(request.args[0], exe.as_os_str(), "the helper is qpac itself, by its absolute path");
    assert_eq!(request.args[1], "--privileged-helper");
    assert_eq!(
        request.notice.as_deref(),
        Some(
            "qpac asks for administrator permission: 9 packages will be installed.\n\
             polkit asks for your password; qpac never sees or keeps it.\n\
             The permission lasts until qpac closes.\n"
        ),
        "qpac's three lines come before pkexec's own"
    );
}

#[test]
fn the_setting_can_insist_on_sudo_even_where_polkit_is() {
    let (mut h, helper, _) = screen("[privilege]\ntool = \"sudo\"\n");
    install_flatpak(&mut h);
    assert!(h.detached_handoffs().is_empty(), "pkexec is not asked");
    assert_eq!(h.handoffs().len(), 1);
    assert_eq!(h.handoffs()[0].program, "sudo");
    assert_eq!(helper.starts(), 1, "the sudo path starts its helper as before");
}

#[test]
fn polkit_asks_in_turkish_too() {
    let (mut h, _, _) = screen("");
    h.set_detached_outcome(DetachedOutcome::Finished { code: Some(126) });
    h.set_locale("tr");
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Kur");
    settle(&mut h);
    assert_eq!(
        h.detached_handoffs()[0].notice.as_deref(),
        Some(
            "qpac yönetici izni istiyor: 9 paket kurulacak.\n\
             Parolanı polkit soracak; qpac parolanı görmez ve saklamaz.\n\
             İzin qpac kapanana kadar geçerli.\n"
        )
    );
    assert!(h.screen().contains("İzin verilmedi"), "{}", h.screen());
}

#[test]
fn a_ready_helper_carries_out_the_transaction_and_the_badge_shows() {
    let (mut h, program) = granted();
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    serve_install(&mut h, &program, 1, 0);
    until(&mut h, "the success toast", |h| h.screen().contains("Installed flatpak"));
    assert_eq!(program.written(), ["size 107 9", "install flatpak"], "the size, then the request");
    assert!(h.app().transaction.has_helper());
    assert!(h.screen().contains("◆ admin"), "{}", h.screen());
    assert!(program.stdin_open(), "the helper stays up for the next transaction");
}

#[test]
fn a_second_transaction_reuses_the_helper_without_asking_again() {
    let (mut h, program) = granted();
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    serve_install(&mut h, &program, 1, 0);
    until(&mut h, "the first success", |h| h.screen().contains("Installed flatpak"));
    ask_to_install_flatpak(&mut h);
    assert!(h.screen().contains("Administrator permission was given for this"), "{}", h.screen());
    click_last(&mut h, "Install");
    serve_install(&mut h, &program, 2, 0);
    until(&mut h, "the second run", |_| program.written().len() == 4);
    until(&mut h, "the second success", |h| !h.app().transaction.shows_output());
    assert_eq!(h.detached_handoffs().len(), 1, "polkit asked once");
    assert!(h.handoffs().is_empty());
    assert_eq!(program.written(), ["size 107 9", "install flatpak", "size 107 9", "install flatpak"]);
}

#[test]
fn a_failed_pacman_through_the_helper_keeps_its_output_and_the_helper() {
    let (mut h, program) = granted();
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    serve_install(&mut h, &program, 1, 1);
    until(&mut h, "the failure toast", |h| h.screen().contains("pacman ended with code 1"));
    assert!(h.screen().contains("(1/1) installing flatpak"), "{}", h.screen());
    assert!(h.app().transaction.has_helper());
}

#[test]
fn a_refused_password_shows_not_authorized_and_changes_nothing() {
    for code in [126, 127] {
        let (mut h, _, recorded) = screen("");
        h.set_detached_outcome(DetachedOutcome::Finished { code: Some(code) });
        let screen = install_flatpak(&mut h);
        assert!(screen.contains("Permission was not given"), "{code}:\n{screen}");
        assert!(screen.contains("Nothing was changed."), "{code}:\n{screen}");
        assert!(!screen.contains("Install 9 packages?"), "the dialog is gone:\n{screen}");
        assert!(screen.contains("Install flatpak"), "the list is back:\n{screen}");
        assert!(!screen.contains("◆ admin"), "{screen}");
        assert!(!h.app().transaction.has_helper());
        assert_eq!(after_reads(&recorded).len(), 1, "only the planning ran");
    }
}

#[test]
fn a_helper_that_ends_before_it_is_ready_changes_nothing() {
    let (mut h, _, _) = screen("");
    h.set_detached_outcome(DetachedOutcome::Finished { code: Some(1) });
    let screen = install_flatpak(&mut h);
    assert!(screen.contains("Permission was not given"), "{screen}");
    assert!(screen.contains("Nothing was changed."), "{screen}");
}

#[test]
fn a_helper_that_refuses_to_serve_is_let_go() {
    let (mut h, _, _) = screen("");
    let (child, program) = LiveChild::for_tests();
    h.set_detached_outcome(DetachedOutcome::Detached { child, first_line: "refused not-root".to_owned() });
    let screen = install_flatpak(&mut h);
    assert!(screen.contains("Permission was not given"), "{screen}");
    assert!(screen.contains("not running as the administrator"), "{screen}");
    assert!(!program.stdin_open(), "its input is closed");
    assert!(program.written().is_empty(), "nothing was asked of it");
    assert!(!h.app().transaction.has_helper());
}

#[test]
fn pkexec_that_cannot_run_says_the_helper_could_not_start() {
    let (mut h, _, _) = screen("");
    h.set_detached_outcome(DetachedOutcome::Failed("No such file or directory (os error 2)".to_owned()));
    let screen = install_flatpak(&mut h);
    assert!(screen.contains("The administrator helper could not be started"), "{screen}");
    assert!(screen.contains("No such file or directory"), "{screen}");
    assert!(!h.app().transaction.has_helper());
}

#[test]
fn letting_the_badge_go_closes_the_helpers_input_and_the_next_change_asks_again() {
    let (mut h, program) = granted();
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    serve_install(&mut h, &program, 1, 0);
    until(&mut h, "the success toast", |h| h.screen().contains("Installed flatpak"));
    let (x, y) = h.find("◆ admin").expect("the badge is on screen");
    h.click(x + 2, y);
    assert!(!program.stdin_open(), "closing its input is how a root helper is asked to end");
    assert!(!program.killed(), "a root helper is never signalled");
    assert!(!h.app().transaction.has_helper());
    assert!(!h.screen().contains("◆ admin"), "{}", h.screen());
    h.set_detached_outcome(DetachedOutcome::Finished { code: Some(126) });
    install_flatpak(&mut h);
    assert_eq!(h.detached_handoffs().len(), 2, "polkit asks again");
}

#[test]
fn a_helper_that_ends_on_its_own_takes_the_badge_with_it() {
    let (mut h, program) = granted();
    ask_to_install_flatpak(&mut h);
    click_last(&mut h, "Install");
    serve_install(&mut h, &program, 1, 0);
    until(&mut h, "the success toast", |h| h.screen().contains("Installed flatpak"));
    program.exit(Some(0));
    until(&mut h, "the badge to go", |h| !h.screen().contains("◆ admin"));
    assert!(!h.app().transaction.has_helper());
}

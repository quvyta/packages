//! Updating the system through the screen: the confirmation with the list the check found, the
//! snapshots around it, the question when one cannot be taken, the list of new `.pacnew` files,
//! and the offer to bring pending updates along with an installation. Nothing reaches pacman or
//! a snapshot tool: the helper plays them from the recording.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::ProcessOutcome;
use qpackages_core::backup::Snapshot;
use qpackages_core::helper::PACMAN_PATH;
use qpackages_core::news::NewsItem;
use qpackages_core::pacman::command::{install, upgrade, upgrade_install};

use super::flow::{click_last, runner, settle};
use super::{app_on, env, fixture, three_updates};
use crate::app::{Msg, Places, Qpackages, Tab};
use crate::helper::session::InProcess;
use crate::runner::Recorded;
use crate::{settings_page, updates};

/// A folder standing in for `/`, with these files; emptied first when a run before stopped.
fn root(name: &str, files: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("qpackages-upgrade-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a scratch root");
    for file in files {
        let path = root.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
        fs::write(path, "").expect("a file");
    }
    root
}

/// A machine with snapper and its `root` configuration.
const SNAPPER: [&str; 2] = ["usr/bin/snapper", "etc/snapper/configs/root"];

/// The screen on the fixture with the system's files under `root`, following `settings`, the
/// three updates of the shared sample found, on the Updates tab.
fn screen(recorded: &Arc<Recorded>, settings: &str, root: &Path) -> Harness<Qpackages> {
    let helper = InProcess::new(recorded, 0);
    let app = app_on(settings, recorded, &helper, Some(1000), &fixture()).with_places(Places {
        root: root.to_path_buf(),
        units: None,
        exe: None,
    });
    let mut h = Harness::with_env(app, env(), 120, 30);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.send(Msg::Updates(updates::Msg::Checked(three_updates())));
    h.send(Msg::Tab(Tab::Updates.index()));
    h
}

/// What the helper ran, as `program arg arg…` lines: pacman and the snapshot tools.
fn helper_runs(recorded: &Recorded) -> Vec<String> {
    recorded
        .command_lines()
        .into_iter()
        .filter(|line| line.starts_with(PACMAN_PATH) || line.starts_with("/usr/bin/snapper"))
        .collect()
}

/// `snapshot` as the helper runs it, as a `program arg arg…` line.
fn snapshot_line(snapshot: Snapshot) -> String {
    let (program, args) = snapshot.command();
    std::iter::once(program.to_owned()).chain(args).collect::<Vec<_>>().join(" ")
}

/// Records `snapshot` printing `lines` and ending with `code`.
fn play_snapshot(recorded: &Recorded, snapshot: Snapshot, lines: &[&str], code: i32) {
    let (program, args) = snapshot.command();
    recorded.play(program, &args, lines, ProcessOutcome::Finished { code: Some(code) });
}

/// A recording where the update succeeds and leaves a `.pacnew` file.
fn upgrading() -> Arc<Recorded> {
    let recorded = Arc::new(Recorded::default());
    let lines = [
        ":: Synchronizing package databases...",
        "(1/3) upgrading linux",
        "warning: /etc/pacman.conf installed as /etc/pacman.conf.pacnew",
        "(3/3) upgrading bash",
    ];
    recorded.play(PACMAN_PATH, &upgrade(), &lines, ProcessOutcome::Finished { code: Some(0) });
    recorded
}

#[test]
fn update_all_lists_the_updates_and_says_how_the_system_is_kept_safe() {
    let recorded = upgrading();
    let root = root("confirm", &SNAPPER);
    let mut h = screen(&recorded, "", &root);
    let screen = h.screen();
    assert!(screen.contains("A snapper snapshot is taken before and after updating."), "{screen}");
    let manual = NewsItem {
        title: "foo >= 2 requires manual intervention".to_owned(),
        link: "https://archlinux.org/news/foo/".to_owned(),
        published: 1_789_999_320,
        manual_intervention: true,
    };
    h.send(Msg::Updates(updates::Msg::News(Ok(vec![manual]))));
    click_last(&mut h, "Update all");
    let screen = h.screen();
    for text in [
        "Update 3 packages?",
        "Arch news: foo >= 2 requires manual intervention",
        "linux  6.18.1-1 → 6.18.2-1",
        "restart needed",
        "bash  5.3.15-1 → 5.3.16-1",
        "A snapper snapshot is taken before and after updating.",
        "Administrator permission will be asked;",
        "Cancel",
        "Update",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(helper_runs(&recorded).is_empty(), "nothing runs before the user confirms");
    h.press("esc");
    assert!(!h.screen().contains("Update 3 packages?"), "{}", h.screen());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn snapper_takes_a_snapshot_before_and_after_and_the_pacnew_files_are_listed() {
    let recorded = upgrading();
    play_snapshot(&recorded, Snapshot::SnapperPre, &["42"], 0);
    play_snapshot(&recorded, Snapshot::SnapperPost(42), &[], 0);
    let root = root("snapper", &SNAPPER);
    let mut h = screen(&recorded, "", &root);
    let news_before = recorded.command_lines().iter().filter(|line| line.starts_with("curl")).count();
    click_last(&mut h, "Update all");
    click_last(&mut h, "Update");
    let screen = settle(&mut h);
    assert_eq!(
        helper_runs(&recorded),
        [
            snapshot_line(Snapshot::SnapperPre),
            format!("{PACMAN_PATH} -Syu --noconfirm"),
            snapshot_line(Snapshot::SnapperPost(42)),
        ]
    );
    for text in ["1 new configuration file", "/etc/pacman.conf.pacnew", "pacdiff"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    click_last(&mut h, "Close");
    let screen = settle(&mut h);
    assert!(!screen.contains("/etc/pacman.conf.pacnew"), "the list closes:\n{screen}");
    let news_after = recorded.command_lines().iter().filter(|line| line.starts_with("curl")).count();
    assert_eq!(news_after, news_before + 1, "the updates are looked for again, with the news");
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_snapshot_that_fails_stops_and_asks_before_anything_is_updated() {
    let recorded = upgrading();
    play_snapshot(&recorded, Snapshot::SnapperPre, &["Creating snapshot failed."], 1);
    let root = root("failed", &SNAPPER);
    let mut h = screen(&recorded, "", &root);
    click_last(&mut h, "Update all");
    click_last(&mut h, "Update");
    let screen = settle(&mut h);
    for text in ["Update without a backup?", "Backup could not be taken: The snapshot tool ended", "Continue without?"]
    {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert_eq!(helper_runs(&recorded), [snapshot_line(Snapshot::SnapperPre)], "the update waits for the answer");
    click_last(&mut h, "Continue without");
    settle(&mut h);
    assert_eq!(
        helper_runs(&recorded),
        [snapshot_line(Snapshot::SnapperPre), format!("{PACMAN_PATH} -Syu --noconfirm")],
        "the update runs, and no snapshot after is paired with a snapshot that is not there"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn stopping_at_the_question_updates_nothing() {
    let recorded = upgrading();
    play_snapshot(&recorded, Snapshot::SnapperPre, &[], 1);
    let root = root("stopped", &SNAPPER);
    let mut h = screen(&recorded, "", &root);
    click_last(&mut h, "Update all");
    click_last(&mut h, "Update");
    settle(&mut h);
    click_last(&mut h, "Don't update");
    let screen = settle(&mut h);
    assert!(screen.contains("The update was not started"), "{screen}");
    assert_eq!(helper_runs(&recorded), [snapshot_line(Snapshot::SnapperPre)]);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_chosen_tool_that_is_missing_is_said_before_the_password_is_asked() {
    let recorded = upgrading();
    let root = root("missing", &[]);
    let mut h = screen(&recorded, "[backup]\ntool = \"timeshift\"\n", &root);
    assert!(h.screen().contains("timeshift is chosen for snapshots but is not installed."), "{}", h.screen());
    click_last(&mut h, "Update all");
    click_last(&mut h, "Update");
    let screen = settle(&mut h);
    assert!(screen.contains("Backup could not be taken: timeshift is not"), "{screen}");
    assert!(h.handoffs().is_empty(), "no password is asked before the answer");
    click_last(&mut h, "Continue without");
    settle(&mut h);
    assert_eq!(helper_runs(&recorded), [format!("{PACMAN_PATH} -Syu --noconfirm")]);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn snap_pac_takes_the_snapshots_so_qpac_does_not() {
    let recorded = upgrading();
    let root = root("snap-pac", &[&SNAPPER[..], &["var/lib/pacman/local/snap-pac-3.0.1-1/desc"]].concat());
    let mut h = screen(&recorded, "", &root);
    assert!(h.screen().contains("snap-pac takes snapper snapshots around every update."), "{}", h.screen());
    click_last(&mut h, "Update all");
    click_last(&mut h, "Update");
    settle(&mut h);
    assert_eq!(helper_runs(&recorded), [format!("{PACMAN_PATH} -Syu --noconfirm")]);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn an_installation_with_updates_waiting_offers_to_bring_them_along() {
    let recorded = runner();
    recorded.play(PACMAN_PATH, &upgrade_install(&["flatpak"]), &["done"], ProcessOutcome::Finished { code: Some(0) });
    let root = root("offer", &[]);
    let mut h = screen(&recorded, "", &root);
    h.send(Msg::Settings(settings_page::Msg::Install(qpackages_core::sources::Source::Flatpak)));
    let screen = h.screen();
    for text in ["3 updates are waiting.", "Cancel", "Install only", "Update the system first and install"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    click_last(&mut h, "Update the system first and install");
    let screen = settle(&mut h);
    assert_eq!(helper_runs(&recorded), [format!("{PACMAN_PATH} -Syu --needed --noconfirm -- flatpak")]);
    assert!(screen.contains("Updated the system and installed flatpak"), "{screen}");
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn install_only_installs_without_the_updates() {
    let recorded = runner();
    recorded.play(PACMAN_PATH, &install(&["flatpak"]), &["done"], ProcessOutcome::Finished { code: Some(0) });
    let root = root("install-only", &[]);
    let mut h = screen(&recorded, "", &root);
    h.send(Msg::Settings(settings_page::Msg::Install(qpackages_core::sources::Source::Flatpak)));
    click_last(&mut h, "Install only");
    settle(&mut h);
    assert_eq!(helper_runs(&recorded), [format!("{PACMAN_PATH} -S --needed --noconfirm -- flatpak")]);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn without_waiting_updates_an_installation_asks_nothing_more() {
    let recorded = runner();
    let helper = InProcess::new(&recorded, 0);
    let mut h = Harness::with_env(app_on("", &recorded, &helper, Some(1000), &fixture()), env(), 120, 30);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    h.send(Msg::Settings(settings_page::Msg::Install(qpackages_core::sources::Source::Flatpak)));
    let screen = h.screen();
    assert!(screen.contains("Install 9 packages?"), "{screen}");
    assert!(!screen.contains("Install only") && !screen.contains("waiting"), "{screen}");
}

#[test]
fn the_confirmation_reads_in_turkish_and_keeps_the_rules_in_ascii_and_narrow() {
    for (width, height) in [(60, 24), (120, 30)] {
        let recorded = upgrading();
        let root = root(&format!("rules-{width}"), &SNAPPER);
        let mut h = screen(&recorded, "", &root);
        h.resize(width, height);
        h.set_glyph_mode(GlyphMode::Ascii);
        click_last(&mut h, "Update all");
        let screen = h.screen();
        assert!(screen.contains("> 6.18.2-1"), "the arrow in ASCII at {width}:\n{screen}");
        for forbidden in ['[', ']', '{', '}', '|'] {
            assert!(!screen.contains(forbidden), "`{forbidden}` at {width}x{height}:\n{screen}");
        }
        h.set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("3 paket güncellensin mi?"), "{screen}");
        assert!(screen.contains("Güncellemeden önce ve sonra snapper yedeği alınır."), "{screen}");
        assert!(!screen.contains('⟦'), "a key is missing in Turkish:\n{screen}");
        h.set_glyph_mode(GlyphMode::Nerd);
        assert!(h.screen().contains("3 paket güncellensin mi?"), "{}", h.screen());
        fs::remove_dir_all(root).expect("cleanup");
    }
}

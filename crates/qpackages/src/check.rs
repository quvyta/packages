//! `qpac --check`: the update check without a screen, for a user timer.
//!
//! It runs as the user and changes nothing on the system: the repositories are refreshed in a
//! private copy of pacman's database, never the real one, and the AUR is asked about every
//! foreign package in as few requests as fit. What it found goes to the state file the screen
//! reads. It stays away from the mirrors when the last successful check is less than an hour
//! old, and from pacman's database while a transaction holds it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use qframe::i18n::{Arg, I18n};
use qframe::storage::Family;
use qpackages_core::catalog::aur::{info_urls, parse_response};
use qpackages_core::catalog::net::{CURL, curl_args};
use qpackages_core::check::{UpdateState, aur_updates, checked_recently};
use qpackages_core::lock::{LockStatus, lock_status};
use qpackages_core::pacman::command::{self, FAKEROOT, PACMAN, parsed_env};
use qpackages_core::pacman::{UpdateCheck, parse_foreign, read_update_check, syncdb};

use crate::runner::{Real, Runner};
use crate::settings::APP;

/// The flag that runs the check instead of the screen.
pub const FLAG: &str = "--check";

/// Where pacman keeps its database.
const PACMAN_DIR: &str = "/var/lib/pacman";

/// Where a check reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// pacman's database folder, holding `sync/`, `local/` and the lock.
    pub pacman_dir: PathBuf,
    /// The private copy of the database, refreshed without privileges.
    pub private_db: PathBuf,
    /// The state file the screen reads.
    pub state_file: PathBuf,
}

impl Places {
    /// The places for the user this process runs as: the state file in the family's state folder
    /// for qpac, the private copy of the database in its cache folder. `None` without a home
    /// folder to keep them in, in which case no check can remember anything.
    #[must_use]
    pub fn for_user() -> Option<Self> {
        let family = Family::QUVYTA;
        Some(Self::under(&family.state_dir(APP)?, &family.cache_dir(APP)?))
    }

    /// The places with `state` and `cache` as the two folders, for a test or a pretend machine
    /// that must leave the user's own folders alone.
    #[must_use]
    pub fn under(state: &Path, cache: &Path) -> Self {
        Self {
            pacman_dir: PathBuf::from(PACMAN_DIR),
            private_db: cache.join("db"),
            state_file: state.join("state.json"),
        }
    }
}

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The last successful check is less than an hour old; nothing was asked.
    Recent,
    /// pacman's database is locked by a transaction; nothing was asked.
    Locked,
    /// The check ran and its findings are in the state file.
    Checked(UpdateState),
    /// The check did not finish; the state file was left as it was. What went wrong, in English,
    /// for the log.
    Failed(String),
}

/// Checks for updates at `now` (seconds since the Unix epoch), running every program through
/// `runner`.
#[must_use]
pub fn check(places: &Places, runner: &dyn Runner, now: i64) -> Outcome {
    let last = fs::read_to_string(&places.state_file).ok().and_then(|text| UpdateState::parse(&text).ok());
    if checked_recently(last.map(|state| state.checked), now) {
        return Outcome::Recent;
    }
    if matches!(lock_status(&places.pacman_dir), LockStatus::Held { .. }) {
        return Outcome::Locked;
    }
    let state = match find_updates(places, runner, now) {
        Ok(state) => state,
        Err(reason) => return Outcome::Failed(reason),
    };
    match write_atomically(&places.state_file, &state.to_json()) {
        Ok(()) => Outcome::Checked(state),
        Err(error) => Outcome::Failed(format!("{}: {error}", places.state_file.display())),
    }
}

/// Refreshes the private copy, lists the repository updates, then asks the AUR about the
/// foreign packages.
fn find_updates(places: &Places, runner: &dyn Runner, now: i64) -> Result<UpdateState, String> {
    let env = parsed_env();
    let db = syncdb::prepare(&places.pacman_dir.join("sync"), &places.private_db)
        .map_err(|error| format!("{}: {error}", places.private_db.display()))?;
    let refreshed = runner.output(FAKEROOT, &command::refresh(&db), &env).map_err(|error| error.to_string())?;
    if !refreshed.succeeded() {
        return Err(refreshed.stderr.trim().to_owned());
    }
    let listed = runner.output(PACMAN, &command::update_check(&db), &env).map_err(|error| error.to_string())?;
    let pacman = match read_update_check(listed.code, &listed.stdout, &listed.stderr) {
        UpdateCheck::Updates(updates) => updates,
        UpdateCheck::Failed { stderr } => return Err(stderr.trim().to_owned()),
    };
    let foreign_out = runner.output(PACMAN, &command::foreign_versions(), &env).map_err(|error| error.to_string())?;
    // pacman -Qm exits 1 when no package is foreign.
    let foreign = match foreign_out.code {
        Some(0) => parse_foreign(&foreign_out.stdout),
        Some(1) if foreign_out.stdout.trim().is_empty() && foreign_out.stderr.trim().is_empty() => Vec::new(),
        _ => return Err(foreign_out.stderr.trim().to_owned()),
    };
    let names: Vec<&str> = foreign.iter().map(|(name, _)| name.as_str()).collect();
    let mut answered = Vec::new();
    for url in info_urls(&names) {
        let fetched = runner.output(CURL, &curl_args(&url), &[]).map_err(|error| error.to_string())?;
        if !fetched.succeeded() {
            return Err(fetched.stderr.trim().to_owned());
        }
        answered.extend(parse_response(&fetched.stdout).map_err(|error| error.to_string())?);
    }
    Ok(UpdateState { checked: now, pacman, aur: aur_updates(&foreign, &answered) })
}

/// Writes `text` to `path` through a file beside it, so the screen reads the old state or the
/// new one, never half of one.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let folder = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(folder)?;
    let fresh = folder.join(".state.json.new");
    fs::write(&fresh, text)?;
    fs::rename(&fresh, path)
}

/// Runs the check on this machine and says how it went on standard error, in the user's
/// language. Returns the process's exit code: 0 when the check ran or had no reason to, 1 when
/// it failed.
#[must_use]
pub fn run() -> i32 {
    let i18n = translator(|name| std::env::var(name).ok());
    let Some(places) = Places::for_user() else {
        eprintln!("{}", i18n.translate("check.no-home", &[]));
        return 1;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX));
    let (message, code) = report(&check(&places, &Real, now), &i18n);
    eprintln!("{message}");
    code
}

/// The line that says how `outcome` went, and the exit code for it.
fn report(outcome: &Outcome, i18n: &I18n) -> (String, i32) {
    match outcome {
        Outcome::Recent => (i18n.translate("check.recent", &[]), 0),
        Outcome::Locked => (i18n.translate("check.locked", &[]), 0),
        Outcome::Checked(state) => {
            let count = |n: usize| Arg::Int(i64::try_from(n).unwrap_or(i64::MAX));
            let args = [("pacman", count(state.pacman.len())), ("aur", count(state.aur.len()))];
            (i18n.translate("check.done", &args), 0)
        }
        Outcome::Failed(reason) => (i18n.translate("check.failed", &[("reason", Arg::Text(reason.clone()))]), 1),
    }
}

/// The compiled-in language files, speaking the language the environment `lookup` asks for.
fn translator(lookup: impl Fn(&str) -> Option<String>) -> I18n {
    let mut i18n = crate::i18n();
    if let Some(code) = i18n.detect(lookup) {
        i18n.set_active(&code);
    }
    i18n
}

#[cfg(test)]
mod tests {
    use qpackages_core::pacman::Update;

    use super::*;
    use crate::runner::Recorded;

    /// 2026-07-25 00:00:00 UTC.
    const NOW: i64 = 1_784_937_600;

    /// A machine under the system's temporary folder: a pacman folder with one repository
    /// database and a local database, and a home for the state and the private copy.
    fn machine(name: &str) -> (Places, PathBuf) {
        let root = std::env::temp_dir().join(format!("qpackages-check-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let pacman_dir = root.join("var/lib/pacman");
        fs::create_dir_all(pacman_dir.join("sync")).expect("sync");
        fs::create_dir_all(pacman_dir.join("local")).expect("local");
        fs::write(pacman_dir.join("sync/core.db"), "core").expect("a database");
        let home = root.join("home");
        let places = Places::under(&home.join("state"), &home.join("cache"));
        (Places { pacman_dir, ..places }, root)
    }

    /// Records a whole successful check: two repository updates and one AUR update.
    fn recorded(places: &Places) -> Recorded {
        let recorded = Recorded::default();
        let db = &places.private_db;
        recorded.answer(FAKEROOT, &command::refresh(db), ":: Synchronizing package databases...\n", 0);
        recorded.answer(
            PACMAN,
            &command::update_check(db),
            "bash 5.3.15-1 -> 5.3.16-1\nlinux 6.18.1-1 -> 6.18.2-1\n",
            0,
        );
        recorded.answer(PACMAN, &command::foreign_versions(), "paru 2.1.0-1\nbrave-bin 1:1.95.101-1\n", 0);
        let url = &info_urls(&["paru", "brave-bin"])[0];
        let answer = r#"{"version":5,"type":"multiinfo","resultcount":2,"results":[
            {"Name":"paru","PackageBase":"paru","Version":"2.1.0-1"},
            {"Name":"brave-bin","PackageBase":"brave-bin","Version":"1:1.95.102-1"}]}"#;
        recorded.answer(CURL, &curl_args(url), answer, 0);
        recorded
    }

    fn update(name: &str, from: &str, to: &str) -> Update {
        Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
    }

    #[test]
    fn a_check_writes_what_it_found_and_never_touches_the_real_database() {
        let (places, root) = machine("found");
        let recorded = recorded(&places);
        let Outcome::Checked(state) = check(&places, &recorded, NOW) else { panic!("the check ran") };
        assert_eq!(state.pacman, [update("bash", "5.3.15-1", "5.3.16-1"), update("linux", "6.18.1-1", "6.18.2-1")]);
        assert_eq!(state.aur, [update("brave-bin", "1:1.95.101-1", "1:1.95.102-1")]);
        let written = fs::read_to_string(&places.state_file).expect("the state file");
        assert_eq!(UpdateState::parse(&written), Ok(state));
        let private = places.private_db.to_string_lossy().into_owned();
        let real = places.pacman_dir.to_string_lossy().into_owned();
        for call in recorded.calls() {
            let line = call.args.join(" ");
            assert!(!line.contains(&real), "`{line}` points pacman at the real database");
            if call.args.iter().any(|arg| arg == "-Sy") {
                assert!(line.contains(&private), "the refresh is aimed at the private copy");
            }
        }
        assert_eq!(recorded.calls().len(), 4, "one refresh, two queries, one AUR request");
        assert_eq!(fs::read_link(places.private_db.join("local")).expect("a link"), places.pacman_dir.join("local"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_check_within_the_hour_asks_nothing() {
        let (places, root) = machine("recent");
        let recorded = recorded(&places);
        assert!(matches!(check(&places, &recorded, NOW), Outcome::Checked(_)));
        let calls = recorded.calls().len();
        assert_eq!(check(&places, &recorded, NOW + 3599), Outcome::Recent);
        assert_eq!(recorded.calls().len(), calls, "nothing was run");
        assert!(matches!(check(&places, &recorded, NOW + 3600), Outcome::Checked(_)), "an hour later it runs again");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_held_lock_skips_the_check() {
        let (places, root) = machine("locked");
        fs::write(places.pacman_dir.join("db.lck"), "").expect("a lock");
        let recorded = recorded(&places);
        assert_eq!(check(&places, &recorded, NOW), Outcome::Locked);
        assert!(recorded.calls().is_empty());
        assert!(!places.state_file.exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_refresh_leaves_the_last_state_alone() {
        let (places, root) = machine("offline");
        let old = UpdateState { checked: NOW - 7200, pacman: vec![update("bash", "1-1", "2-1")], aur: Vec::new() };
        write_atomically(&places.state_file, &old.to_json()).expect("an old state");
        let recorded = Recorded::default();
        recorded.fail(FAKEROOT, &command::refresh(&places.private_db), "error: failed to synchronize all databases", 1);
        let outcome = check(&places, &recorded, NOW);
        assert_eq!(outcome, Outcome::Failed("error: failed to synchronize all databases".to_owned()));
        assert_eq!(UpdateState::parse(&fs::read_to_string(&places.state_file).expect("kept")), Ok(old));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn no_foreign_package_asks_the_aur_nothing() {
        let (places, root) = machine("no-aur");
        let recorded = Recorded::default();
        let db = &places.private_db;
        recorded.answer(FAKEROOT, &command::refresh(db), "", 0);
        recorded.answer(PACMAN, &command::update_check(db), "", 1);
        recorded.answer(PACMAN, &command::foreign_versions(), "", 1);
        let Outcome::Checked(state) = check(&places, &recorded, NOW) else { panic!("the check ran") };
        assert!(state.pacman.is_empty() && state.aur.is_empty());
        assert!(recorded.command_lines().iter().all(|line| !line.starts_with("curl")));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_aur_request_fails_the_check() {
        let (places, root) = machine("aur-down");
        let recorded = recorded(&places);
        let url = &info_urls(&["paru", "brave-bin"])[0];
        recorded.fail(CURL, &curl_args(url), "curl: (22) The requested URL returned error: 503", 22);
        let outcome = check(&places, &recorded, NOW);
        assert_eq!(outcome, Outcome::Failed("curl: (22) The requested URL returned error: 503".to_owned()));
        assert!(!places.state_file.exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_state_file_and_the_private_copy_are_two_folders_apart() {
        let places = Places::under(Path::new("/st/quvyta/packages"), Path::new("/ca/quvyta/packages"));
        assert_eq!(places.state_file, Path::new("/st/quvyta/packages/state.json"));
        assert_eq!(places.private_db, Path::new("/ca/quvyta/packages/db"));
        assert_eq!(places.pacman_dir, Path::new(PACMAN_DIR), "pacman's own database is never the private one");
    }

    #[test]
    fn the_user_keeps_them_in_the_family_folders() {
        // Reads the environment, writes nothing: the folders are the family's, under qpac's name.
        let Some(places) = Places::for_user() else { return };
        assert!(places.state_file.ends_with("quvyta/packages/state.json"), "{}", places.state_file.display());
        assert!(places.private_db.ends_with("quvyta/packages/db"), "{}", places.private_db.display());
        assert_ne!(
            places.state_file.parent(),
            places.private_db.parent(),
            "state is kept and a cache is thrown away, so they never share a folder"
        );
    }

    #[test]
    fn every_outcome_is_said_in_both_languages() {
        let state = UpdateState { checked: NOW, pacman: vec![update("a", "1", "2")], aur: Vec::new() };
        for language in ["en_US.UTF-8", "tr_TR.UTF-8"] {
            let i18n = translator(|name| (name == "LANG").then(|| language.to_owned()));
            for (outcome, code) in [
                (Outcome::Recent, 0),
                (Outcome::Locked, 0),
                (Outcome::Checked(state.clone()), 0),
                (Outcome::Failed("offline".to_owned()), 1),
            ] {
                let (message, exit) = report(&outcome, &i18n);
                assert!(!message.contains('⟦'), "{language}: {message}");
                assert_eq!(exit, code);
            }
            let (done, _) = report(&Outcome::Checked(state.clone()), &i18n);
            assert!(done.contains('1') && done.contains('0'), "{done}");
            assert!(!i18n.translate("check.no-home", &[]).contains('⟦'));
        }
    }
}

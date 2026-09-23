//! `qpac --check`: the update check without a screen, for a user timer.
//!
//! It runs as the user and changes nothing on the system: the repositories are refreshed in a
//! private copy of pacman's database, never the real one, and the AUR is asked about every
//! foreign package in as few requests as fit. What it found goes to the state file the screen
//! reads. It stays away from the mirrors when the last successful check is less than an hour
//! old, and from pacman's database while a transaction holds it.
//!
//! What it does next is the ladder's step the person chose (`updates.mode`): nothing more when
//! told to tell; the repositories' updates downloaded ahead into a folder of the user's, still
//! without root, when told to download; and those updates installed through the root-owned script
//! when told to install and the script is there.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use qframe::i18n::{Arg, I18n};
use qframe::storage::Family;
use qpackages_core::catalog::aur::{info_urls, parse_response};
use qpackages_core::catalog::net::{CURL, curl_args};
use qpackages_core::check::{UpdateState, aur_updates, checked_recently};
use qpackages_core::helper::DOWNLOADS;
use qpackages_core::lock::{LockStatus, lock_status};
use qpackages_core::pacman::command::{self, FAKEROOT, PACMAN, SUDO, parsed_env};
use qpackages_core::pacman::{Update, UpdateCheck, parse_foreign, read_update_check, syncdb};

use crate::ladder::{self, Mode};
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
    /// Where the repositories' updates are downloaded ahead, a folder of the user's own.
    pub downloads: PathBuf,
    /// The root-owned script the install step runs.
    pub script: PathBuf,
}

impl Places {
    /// The places for the user this process runs as: the state file in the family's state folder
    /// for qpac, the private copy of the database in its cache folder. `None` without a home
    /// folder to keep them in, in which case no check can remember anything.
    #[must_use]
    pub fn for_user() -> Option<Self> {
        let family = Family::QUVYTA;
        let places = Self::under(&family.state_dir(APP)?, &family.cache_dir(APP)?);
        // The downloads are under the home folder itself, whatever `XDG_CACHE_HOME` says: the
        // helper that takes them runs as root and finds the folder only through the password
        // database.
        let home = PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?);
        Some(Self { downloads: home.join(DOWNLOADS), ..places })
    }

    /// The places with `state` and `cache` as the two folders, for a test or a pretend machine
    /// that must leave the user's own folders alone.
    #[must_use]
    pub fn under(state: &Path, cache: &Path) -> Self {
        Self {
            pacman_dir: PathBuf::from(PACMAN_DIR),
            private_db: cache.join("db"),
            state_file: state.join("state.json"),
            downloads: cache.join("downloads"),
            script: PathBuf::from(ladder::SCRIPT),
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
    /// The check ran and its findings are in the state file; what the ladder's step did after.
    Checked(UpdateState, Fetched),
    /// The check did not finish; the state file was left as it was. What went wrong, in English,
    /// for the log.
    Failed(String),
}

/// What the ladder's step did once the updates were found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// Nothing: the step is to tell, or there was nothing from the repositories to fetch.
    Nothing,
    /// This many packages from the repositories are downloaded and wait for the next update.
    Downloaded(usize),
    /// Downloading ahead did not finish; what pacman said.
    DownloadFailed(String),
    /// The script installed the repositories' updates.
    Installed,
    /// The script did not install them; what sudo or the script said.
    InstallFailed(String),
}

/// Checks for updates at `now` (seconds since the Unix epoch), running every program through
/// `runner`, and then does what the ladder's step `mode` asks. `mode` is what runs on this
/// machine ([`Mode::effective`]): install only where the script was found trusted.
#[must_use]
pub fn check(places: &Places, runner: &dyn Runner, now: i64, mode: Mode) -> Outcome {
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
    if let Err(error) = write_atomically(&places.state_file, &state.to_json()) {
        return Outcome::Failed(format!("{}: {error}", places.state_file.display()));
    }
    let (state, fetched) = climb(places, runner, state, mode);
    Outcome::Checked(state, fetched)
}

/// Does what the ladder's step `mode` asks with the updates in `state`, and returns the state as
/// it stands after: fewer updates once the script installed them.
///
/// The downloads folder only ever holds the pending updates: what an earlier check fetched and
/// is no longer pending goes first, and when the step is to tell, or nothing is pending, all of
/// it goes, so turning the step down frees the space.
fn climb(places: &Places, runner: &dyn Runner, mut state: UpdateState, mode: Mode) -> (UpdateState, Fetched) {
    let pending = if mode == Mode::Notify { &[][..] } else { &state.pacman[..] };
    keep_only(&places.downloads, pending);
    if pending.is_empty() {
        return (state, Fetched::Nothing);
    }
    let env = parsed_env();
    if mode == Mode::Install {
        let ran = runner.output(SUDO, &install_args(&places.script), &env);
        return match ran {
            Ok(done) if done.succeeded() => {
                // The private copy is still fresh, so what is left is asked of it without the
                // network; a failed question leaves the list as it was found.
                if let Ok(listed) = runner.output(PACMAN, &command::update_check(&places.private_db), &env)
                    && let UpdateCheck::Updates(left) = read_update_check(listed.code, &listed.stdout, &listed.stderr)
                {
                    state.pacman = left;
                    let _ = write_atomically(&places.state_file, &state.to_json());
                }
                (state, Fetched::Installed)
            }
            Ok(done) => {
                let said = if done.stderr.trim().is_empty() { done.stdout } else { done.stderr };
                (state, Fetched::InstallFailed(said.trim().to_owned()))
            }
            Err(error) => (state, Fetched::InstallFailed(error.to_string())),
        };
    }
    if let Err(error) = fs::create_dir_all(&places.downloads) {
        return (state, Fetched::DownloadFailed(format!("{}: {error}", places.downloads.display())));
    }
    let fetched = match runner.output(FAKEROOT, &command::download(&places.private_db, &places.downloads), &env) {
        Ok(done) if done.succeeded() => Fetched::Downloaded(packages_in(&places.downloads, &state.pacman)),
        Ok(done) => Fetched::DownloadFailed(done.stderr.trim().to_owned()),
        Err(error) => Fetched::DownloadFailed(error.to_string()),
    };
    (state, fetched)
}

/// The arguments that run the install step's `script` through sudo without ever asking for a
/// password: `-n` makes sudo fail at once where no sudoers line allows the script, since nobody
/// is there to type one.
fn install_args(script: &Path) -> Vec<String> {
    vec!["-n".to_owned(), script.to_string_lossy().into_owned()]
}

/// The start `pacman -Sw` gives the file names of `update`: `name-version-`, followed by the
/// architecture.
fn file_start(update: &Update) -> String {
    format!("{}-{}-", update.name, update.to)
}

/// Removes every file in `folder` that is not a download of one of `pending`; a missing folder is
/// nothing to do. Folders pacman may leave behind are left alone.
fn keep_only(folder: &Path, pending: &[Update]) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    let starts: Vec<String> = pending.iter().map(file_start).collect();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let wanted = starts.iter().any(|start| name.starts_with(start.as_str()));
        if !wanted && entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// How many of `pending` have their package file in `folder`.
fn packages_in(folder: &Path, pending: &[Update]) -> usize {
    let names: Vec<String> = fs::read_dir(folder)
        .map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    pending
        .iter()
        .filter(|update| {
            let start = file_start(update);
            names.iter().any(|name| name.starts_with(&start) && !name.ends_with(".sig"))
        })
        .count()
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
    let (message, code) = report(&check_with(&crate::settings::for_check(), &places, &Real, now), &i18n);
    eprintln!("{message}");
    code
}

/// [`check`] at the ladder's step `settings` choose, as far as this machine allows.
#[must_use]
pub fn check_with(settings: &qframe::storage::Settings, places: &Places, runner: &dyn Runner, now: i64) -> Outcome {
    check(places, runner, now, mode_on_this_machine(settings))
}

/// The ladder's step `settings` choose, as it can run on this machine: install only where the
/// root-owned script is there and trusted.
#[must_use]
pub fn mode_on_this_machine(settings: &qframe::storage::Settings) -> Mode {
    crate::backend_settings::update_mode(settings).effective(ladder::script_under(Path::new("/")).is_some())
}

/// The line that says how `outcome` went, and the exit code for it.
fn report(outcome: &Outcome, i18n: &I18n) -> (String, i32) {
    match outcome {
        Outcome::Recent => (i18n.translate("check.recent", &[]), 0),
        Outcome::Locked => (i18n.translate("check.locked", &[]), 0),
        Outcome::Checked(state, fetched) => {
            let count = |n: usize| Arg::Int(i64::try_from(n).unwrap_or(i64::MAX));
            let args = [("pacman", count(state.pacman.len())), ("aur", count(state.aur.len()))];
            let found = i18n.translate("check.done", &args);
            let reason = |said: &String| [("reason", Arg::Text(said.clone()))];
            let (after, code) = match fetched {
                Fetched::Nothing => return (found, 0),
                Fetched::Downloaded(n) => (i18n.translate("check.downloaded", &[("n", count(*n))]), 0),
                Fetched::DownloadFailed(said) => (i18n.translate("check.download-failed", &reason(said)), 1),
                Fetched::Installed => (i18n.translate("check.installed", &[]), 0),
                Fetched::InstallFailed(said) => (i18n.translate("check.install-failed", &reason(said)), 1),
            };
            (format!("{found} {after}"), code)
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
pub(crate) mod tests {
    use super::*;
    use crate::runner::Recorded;

    /// 2026-07-25 00:00:00 UTC.
    pub(crate) const NOW: i64 = 1_784_937_600;

    /// A machine under the system's temporary folder: a pacman folder with one repository
    /// database and a local database, and a home for the state and the private copy.
    pub(crate) fn machine(name: &str) -> (Places, PathBuf) {
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
    pub(crate) fn recorded(places: &Places) -> Recorded {
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

    fn update(name: &str, from: &str, to: &str) -> qpackages_core::pacman::Update {
        Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
    }

    #[test]
    fn a_check_writes_what_it_found_and_never_touches_the_real_database() {
        let (places, root) = machine("found");
        let recorded = recorded(&places);
        let Outcome::Checked(state, _) = check(&places, &recorded, NOW, Mode::Notify) else { panic!("the check ran") };
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
        assert!(matches!(check(&places, &recorded, NOW, Mode::Notify), Outcome::Checked(..)));
        let calls = recorded.calls().len();
        assert_eq!(check(&places, &recorded, NOW + 3599, Mode::Notify), Outcome::Recent);
        assert_eq!(recorded.calls().len(), calls, "nothing was run");
        assert!(
            matches!(check(&places, &recorded, NOW + 3600, Mode::Notify), Outcome::Checked(..)),
            "an hour later it runs again"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_held_lock_skips_the_check() {
        let (places, root) = machine("locked");
        fs::write(places.pacman_dir.join("db.lck"), "").expect("a lock");
        let recorded = recorded(&places);
        assert_eq!(check(&places, &recorded, NOW, Mode::Notify), Outcome::Locked);
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
        let outcome = check(&places, &recorded, NOW, Mode::Notify);
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
        let Outcome::Checked(state, _) = check(&places, &recorded, NOW, Mode::Notify) else { panic!("the check ran") };
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
        let outcome = check(&places, &recorded, NOW, Mode::Notify);
        assert_eq!(outcome, Outcome::Failed("curl: (22) The requested URL returned error: 503".to_owned()));
        assert!(!places.state_file.exists());
        fs::remove_dir_all(root).ok();
    }

    /// Files in the downloads folder, by name, in order.
    fn downloads(places: &Places) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&places.downloads) else { return Vec::new() };
        let mut names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// Puts `names` in the downloads folder, as pacman or an earlier check would have left them.
    fn left(places: &Places, names: &[&str]) {
        fs::create_dir_all(&places.downloads).expect("the folder");
        for name in names {
            fs::write(places.downloads.join(name), name).expect("a download");
        }
    }

    #[test]
    fn download_fetches_the_repositories_updates_into_the_users_folder_after_the_check() {
        let (places, root) = machine("download");
        let recorded = recorded(&places);
        recorded.answer(FAKEROOT, &command::download(&places.private_db, &places.downloads), "", 0);
        // What pacman downloaded, which a recording cannot write, and what an earlier check left.
        left(&places, &["bash-5.3.16-1-x86_64.pkg.tar.zst", "bash-5.3.16-1-x86_64.pkg.tar.zst.sig"]);
        left(&places, &["bash-5.3.15-1-x86_64.pkg.tar.zst", "vim-9.1-1-x86_64.pkg.tar.zst"]);
        let Outcome::Checked(state, fetched) = check(&places, &recorded, NOW, Mode::Download) else {
            panic!("the check ran")
        };
        assert_eq!(state.pacman.len(), 2);
        assert_eq!(fetched, Fetched::Downloaded(1), "bash is there, linux is not");
        let lines = recorded.command_lines();
        let fetch = format!(
            "fakeroot -- pacman -Suw --noconfirm --dbpath {} --cachedir {} --logfile /dev/null --disable-sandbox",
            places.private_db.display(),
            places.downloads.display()
        );
        assert_eq!(lines.last(), Some(&fetch), "{lines:?}");
        assert!(lines.iter().all(|line| !line.starts_with("sudo")), "nothing runs as root: {lines:?}");
        assert_eq!(
            downloads(&places),
            ["bash-5.3.16-1-x86_64.pkg.tar.zst", "bash-5.3.16-1-x86_64.pkg.tar.zst.sig"],
            "what is no longer pending is gone"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn notify_downloads_nothing_and_empties_what_an_earlier_step_left() {
        let (places, root) = machine("notify");
        let recorded = recorded(&places);
        left(&places, &["bash-5.3.16-1-x86_64.pkg.tar.zst"]);
        let Outcome::Checked(_, fetched) = check(&places, &recorded, NOW, Mode::Notify) else { panic!("ran") };
        assert_eq!(fetched, Fetched::Nothing);
        assert_eq!(recorded.calls().len(), 4, "the check and nothing after it");
        assert_eq!(downloads(&places), Vec::<String>::new());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_download_keeps_what_the_check_found_and_says_so() {
        let (places, root) = machine("download-failed");
        let recorded = recorded(&places);
        let args = command::download(&places.private_db, &places.downloads);
        recorded.fail(FAKEROOT, &args, "error: failed retrieving file 'bash-5.3.16-1-x86_64.pkg.tar.zst'", 1);
        let outcome = check(&places, &recorded, NOW, Mode::Download);
        let Outcome::Checked(state, Fetched::DownloadFailed(said)) = &outcome else { panic!("{outcome:?}") };
        assert!(said.contains("failed retrieving"), "{said}");
        assert_eq!(UpdateState::parse(&fs::read_to_string(&places.state_file).expect("written")).as_ref(), Ok(state));
        let i18n = translator(|name| (name == "LANG").then(|| "en_US.UTF-8".to_owned()));
        assert_eq!(report(&outcome, &i18n).1, 1, "the timer's run shows as failed");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn install_runs_the_script_without_a_password_and_writes_what_is_left() {
        let (places, root) = machine("install");
        let recorded = recorded(&places);
        let db = &places.private_db;
        recorded.answer(SUDO, &install_args(&places.script), "upgrading bash...\nupgrading linux...\n", 0);
        let Outcome::Checked(state, fetched) = check(&places, &recorded, NOW, Mode::Install) else { panic!("ran") };
        assert_eq!(fetched, Fetched::Installed);
        let lines = recorded.command_lines();
        let script = format!("sudo -n {}", ladder::SCRIPT);
        assert!(lines.contains(&script), "{lines:?}");
        assert!(!lines.iter().any(|line| line.contains("-Suw")), "the script downloads for itself: {lines:?}");
        // The recording answers the second question as it answered the first; a real pacman would
        // say nothing is left. What matters is that the state was asked again after the script.
        let after = lines.iter().position(|line| *line == script).expect("ran");
        assert!(lines[after..].contains(&format!("pacman -Qu --dbpath {}", db.display())), "{lines:?}");
        assert_eq!(state.pacman.len(), 2);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn install_without_a_sudoers_line_fails_at_once_and_installs_nothing() {
        let (places, root) = machine("install-refused");
        let recorded = recorded(&places);
        recorded.fail(SUDO, &install_args(&places.script), "sudo: a password is required", 1);
        let outcome = check(&places, &recorded, NOW, Mode::Install);
        assert_eq!(
            outcome,
            Outcome::Checked(
                UpdateState::parse(&fs::read_to_string(&places.state_file).expect("written")).expect("valid"),
                Fetched::InstallFailed("sudo: a password is required".to_owned())
            )
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn nothing_pending_fetches_nothing_whatever_the_step() {
        let (places, root) = machine("nothing");
        for mode in [Mode::Download, Mode::Install] {
            let recorded = Recorded::default();
            let db = &places.private_db;
            recorded.answer(FAKEROOT, &command::refresh(db), "", 0);
            recorded.answer(PACMAN, &command::update_check(db), "", 1);
            recorded.answer(PACMAN, &command::foreign_versions(), "", 1);
            left(&places, &["bash-5.3.16-1-x86_64.pkg.tar.zst"]);
            let Outcome::Checked(_, fetched) =
                check(&places, &recorded, NOW + if mode == Mode::Install { 7200 } else { 0 }, mode)
            else {
                panic!("ran")
            };
            assert_eq!(fetched, Fetched::Nothing);
            assert_eq!(recorded.calls().len(), 3, "{mode:?}");
            assert_eq!(downloads(&places), Vec::<String>::new(), "{mode:?}");
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_state_file_and_the_private_copy_are_two_folders_apart() {
        let places = Places::under(Path::new("/st/quvyta/packages"), Path::new("/ca/quvyta/packages"));
        assert_eq!(places.state_file, Path::new("/st/quvyta/packages/state.json"));
        assert_eq!(places.private_db, Path::new("/ca/quvyta/packages/db"));
        assert_eq!(places.pacman_dir, Path::new(PACMAN_DIR), "pacman's own database is never the private one");
        assert_eq!(places.script, Path::new(ladder::SCRIPT));
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
                (Outcome::Checked(state.clone(), Fetched::Nothing), 0),
                (Outcome::Checked(state.clone(), Fetched::Downloaded(1)), 0),
                (Outcome::Checked(state.clone(), Fetched::DownloadFailed("offline".to_owned())), 1),
                (Outcome::Checked(state.clone(), Fetched::Installed), 0),
                (Outcome::Checked(state.clone(), Fetched::InstallFailed("a password is required".to_owned())), 1),
                (Outcome::Failed("offline".to_owned()), 1),
            ] {
                let (message, exit) = report(&outcome, &i18n);
                assert!(!message.contains('⟦'), "{language}: {message}");
                assert_eq!(exit, code);
            }
            let (done, _) = report(&Outcome::Checked(state.clone(), Fetched::Nothing), &i18n);
            assert!(done.contains('1') && done.contains('0'), "{done}");
            assert!(!i18n.translate("check.no-home", &[]).contains('⟦'));
        }
    }
}

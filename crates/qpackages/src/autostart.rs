//! The user timer that runs `qpac --check` in the background.
//!
//! The units are the user's own files under `$XDG_CONFIG_HOME/systemd/user/`, written and
//! enabled with `systemctl --user`, so nothing here needs privileges. Turning the setting off
//! disables the timer and removes both files.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::check::FLAG;
use crate::runner::Runner;

/// The service the timer starts.
pub const SERVICE: &str = "quvyta-packages-check.service";

/// The timer.
pub const TIMER: &str = "quvyta-packages-check.timer";

/// The program that manages the user's units, found on the search path like the other programs
/// qpac runs as the user.
pub const SYSTEMCTL: &str = "systemctl";

/// How long after login the first check waits, in minutes: a session that just started has
/// better things to do, and every user checking at the same moment would hit the mirrors at once.
const STARTUP_DELAY_MINUTES: u32 = 5;

/// The most every check is moved at random, in minutes, so checks spread over the mirrors.
const RANDOM_DELAY_MINUTES: u32 = 30;

/// The user's unit folder: `$XDG_CONFIG_HOME/systemd/user` when that is an absolute path, else
/// `~/.config/systemd/user`. `None` without a home folder.
#[must_use]
pub fn unit_dir(lookup: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let absolute = |value: String| Some(PathBuf::from(value)).filter(|path| path.is_absolute());
    let config = lookup("XDG_CONFIG_HOME")
        .and_then(absolute)
        .or_else(|| lookup("HOME").and_then(absolute).map(|home| home.join(".config")))?;
    Some(config.join("systemd/user"))
}

/// The service unit that runs `exe --check` once, at low priority; `None` when `exe` cannot be
/// written into a unit (not text, or holding a line break).
#[must_use]
pub fn service_unit(exe: &Path) -> Option<String> {
    let exe = exe.to_str().filter(|path| path.starts_with('/') && !path.contains(|c: char| c.is_control()))?;
    // systemd reads `%` as the start of a specifier and splits the command line on spaces
    // outside quotes.
    let escaped = exe.replace('%', "%%").replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!(
        "# Written by qpac. Turning the background check off in qpac removes this file.\n\
         [Unit]\n\
         Description=Look for package updates\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart=\"{escaped}\" {FLAG}\n\
         Nice=19\n\
         IOSchedulingClass=idle\n"
    ))
}

/// The timer unit: a first check a few minutes after login, then one every `hours` hours, each
/// moved by up to half an hour at random. `hours` is held to 1–168, the range the setting allows.
#[must_use]
pub fn timer_unit(hours: u32) -> String {
    let hours = hours.clamp(1, 168);
    format!(
        "# Written by qpac. Turning the background check off in qpac removes this file.\n\
         [Unit]\n\
         Description=Look for package updates regularly\n\
         \n\
         [Timer]\n\
         OnStartupSec={STARTUP_DELAY_MINUTES}min\n\
         OnUnitActiveSec={hours}h\n\
         RandomizedDelaySec={RANDOM_DELAY_MINUTES}min\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

/// The arguments that make systemd read the unit files again.
#[must_use]
pub fn reload_args() -> Vec<String> {
    ["--user", "daemon-reload"].map(str::to_owned).to_vec()
}

/// The arguments that enable the timer and start it now.
#[must_use]
pub fn enable_args() -> Vec<String> {
    ["--user", "enable", "--now", "--", TIMER].map(str::to_owned).to_vec()
}

/// The arguments that stop the timer and disable it.
#[must_use]
pub fn disable_args() -> Vec<String> {
    ["--user", "disable", "--now", "--", TIMER].map(str::to_owned).to_vec()
}

/// Why the timer could not be switched.
#[derive(Debug)]
pub enum SwitchError {
    /// The program's own path cannot be written into a unit.
    Path,
    /// A unit file could not be written or removed.
    File(io::Error),
    /// systemctl failed; what it said.
    Systemctl(String),
}

impl fmt::Display for SwitchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path => formatter.write_str("the program's path cannot be written into a unit"),
            Self::File(error) => write!(formatter, "a unit file: {error}"),
            Self::Systemctl(said) => write!(formatter, "systemctl: {said}"),
        }
    }
}

impl std::error::Error for SwitchError {}

/// Writes both units into `dir` for the program at `exe`, checking every `hours` hours, then
/// enables and starts the timer through `runner`. Writing them again with another interval
/// replaces them.
///
/// # Errors
///
/// Returns what stopped it; files written before a failed systemctl call stay, and switching
/// off removes them.
pub fn switch_on(dir: &Path, exe: &Path, hours: u32, runner: &dyn Runner) -> Result<(), SwitchError> {
    let service = service_unit(exe).ok_or(SwitchError::Path)?;
    write_atomically(&dir.join(SERVICE), &service).map_err(SwitchError::File)?;
    write_atomically(&dir.join(TIMER), &timer_unit(hours)).map_err(SwitchError::File)?;
    systemctl(runner, &reload_args())?;
    systemctl(runner, &enable_args())
}

/// Stops and disables the timer through `runner`, then removes both units from `dir`. Units
/// that are already gone are not an error.
///
/// # Errors
///
/// Returns what stopped it; when systemctl cannot disable the timer the files stay, so the timer
/// is never left enabled without them.
pub fn switch_off(dir: &Path, runner: &dyn Runner) -> Result<(), SwitchError> {
    if dir.join(TIMER).exists() {
        systemctl(runner, &disable_args())?;
    }
    for unit in [TIMER, SERVICE] {
        match fs::remove_file(dir.join(unit)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(SwitchError::File(error)),
        }
    }
    systemctl(runner, &reload_args())
}

fn systemctl(runner: &dyn Runner, args: &[String]) -> Result<(), SwitchError> {
    let output = runner.output(SYSTEMCTL, args, &[]).map_err(|error| SwitchError::Systemctl(error.to_string()))?;
    if output.succeeded() { Ok(()) } else { Err(SwitchError::Systemctl(output.stderr.trim().to_owned())) }
}

/// Writes `text` to `path` through a file beside it, so systemd never reads half a unit.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let folder = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(folder)?;
    let fresh = folder.join(".quvyta-packages-check.new");
    fs::write(&fresh, text)?;
    fs::rename(&fresh, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Recorded;

    /// A unit folder under the system's temporary folder; never the user's own.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-autostart-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("systemd/user")
    }

    fn systemctl_answers() -> Recorded {
        let recorded = Recorded::default();
        recorded.answer(SYSTEMCTL, &reload_args(), "", 0);
        recorded.answer(SYSTEMCTL, &enable_args(), "", 0);
        recorded.answer(SYSTEMCTL, &disable_args(), "", 0);
        recorded
    }

    #[test]
    fn the_units_run_the_check_on_the_designed_schedule() {
        let service = service_unit(Path::new("/usr/bin/qpac")).expect("a plain path");
        assert!(service.contains("\n[Service]\nType=oneshot\nExecStart=\"/usr/bin/qpac\" --check\n"), "{service}");
        let timer = timer_unit(6);
        let lines: Vec<&str> = timer.lines().filter(|line| line.contains('=')).collect();
        assert_eq!(
            lines,
            [
                "Description=Look for package updates regularly",
                "OnStartupSec=5min",
                "OnUnitActiveSec=6h",
                "RandomizedDelaySec=30min",
                "WantedBy=timers.target"
            ]
        );
        assert!(timer_unit(0).contains("OnUnitActiveSec=1h\n"), "never more often than hourly");
        assert!(timer_unit(1000).contains("OnUnitActiveSec=168h\n"));
    }

    #[test]
    fn an_odd_program_path_is_escaped_or_refused() {
        let service = service_unit(Path::new("/home/a b/.cargo/bin/100%\"q\\")).expect("escaped");
        assert!(service.contains("ExecStart=\"/home/a b/.cargo/bin/100%%\\\"q\\\\\" --check\n"), "{service}");
        assert_eq!(service_unit(Path::new("/x\ny")), None);
        assert_eq!(service_unit(Path::new("relative/qpac")), None);
    }

    #[test]
    fn switching_on_writes_both_units_then_enables_the_timer() {
        let dir = folder("on");
        let recorded = systemctl_answers();
        switch_on(&dir, Path::new("/usr/bin/qpac"), 12, &recorded).expect("switched on");
        assert!(fs::read_to_string(dir.join(SERVICE)).expect("the service").contains("--check"));
        assert!(fs::read_to_string(dir.join(TIMER)).expect("the timer").contains("OnUnitActiveSec=12h"));
        assert_eq!(
            recorded.command_lines(),
            ["systemctl --user daemon-reload", "systemctl --user enable --now -- quvyta-packages-check.timer"]
        );
        fs::remove_dir_all(dir.parent().and_then(Path::parent).expect("the temporary root")).ok();
    }

    #[test]
    fn switching_off_disables_the_timer_then_removes_both_units() {
        let dir = folder("off");
        let recorded = systemctl_answers();
        switch_on(&dir, Path::new("/usr/bin/qpac"), 6, &recorded).expect("switched on");
        switch_off(&dir, &recorded).expect("switched off");
        assert!(!dir.join(SERVICE).exists() && !dir.join(TIMER).exists());
        assert_eq!(
            recorded.command_lines()[2..],
            ["systemctl --user disable --now -- quvyta-packages-check.timer", "systemctl --user daemon-reload"]
        );
        let again = systemctl_answers();
        switch_off(&dir, &again).expect("nothing to switch off is fine");
        assert_eq!(again.command_lines(), ["systemctl --user daemon-reload"], "no timer to disable");
        fs::remove_dir_all(dir.parent().and_then(Path::parent).expect("the temporary root")).ok();
    }

    #[test]
    fn a_failed_systemctl_is_reported_and_keeps_the_files() {
        let dir = folder("failed");
        let recorded = Recorded::default();
        recorded.answer(SYSTEMCTL, &reload_args(), "", 0);
        recorded.fail(SYSTEMCTL, &enable_args(), "Failed to connect to bus: No medium found", 1);
        recorded.fail(SYSTEMCTL, &disable_args(), "Failed to connect to bus: No medium found", 1);
        let error = switch_on(&dir, Path::new("/usr/bin/qpac"), 6, &recorded).expect_err("no user bus");
        assert_eq!(error.to_string(), "systemctl: Failed to connect to bus: No medium found");
        let error = switch_off(&dir, &recorded).expect_err("still no user bus");
        assert!(matches!(error, SwitchError::Systemctl(_)));
        assert!(dir.join(TIMER).exists(), "the timer is never left enabled without its file");
        let missing = Recorded::default();
        assert!(matches!(switch_on(&dir, Path::new("/usr/bin/qpac"), 6, &missing), Err(SwitchError::Systemctl(_))));
        fs::remove_dir_all(dir.parent().and_then(Path::parent).expect("the temporary root")).ok();
    }

    #[test]
    fn the_unit_folder_follows_xdg_config_home() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
        };
        assert_eq!(
            unit_dir(vars(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/c")])),
            Some(PathBuf::from("/c/systemd/user"))
        );
        assert_eq!(
            unit_dir(vars(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "c")])),
            Some(PathBuf::from("/h/.config/systemd/user"))
        );
        assert_eq!(unit_dir(vars(&[])), None);
    }
}

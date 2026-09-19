//! The line protocol between qpackages and its root helper.
//!
//! The helper is qpackages itself, started as root with [`FLAG`]. It reads one request per line
//! on its standard input and answers line by line on its standard output. Everything here is
//! plain text parsed by hand: the helper runs as root, so what it accepts is kept small enough
//! to read in one sitting, and every value is checked before anything runs.

use std::fmt;
use std::path::Path;

use crate::backup::Snapshot;
use crate::pacman::command;
use crate::reflector::Mirrors;

#[cfg(test)]
mod tests;

/// The protocol version the helper announces in its first line.
pub const VERSION: u32 = 1;

/// The command-line flag that starts qpackages as the helper instead of the screen.
pub const FLAG: &str = "--privileged-helper";

/// The flag that gives the helper the language pacman's shown output should use.
pub const LANG_FLAG: &str = "--lang";

/// pacman by its absolute path: the helper never looks a program up on a search path.
pub const PACMAN_PATH: &str = "/usr/bin/pacman";

/// systemctl by its absolute path, for the one timer the helper may switch.
pub const SYSTEMCTL_PATH: &str = "/usr/bin/systemctl";

/// The only unit `timer` accepts: reflector's own timer, which refreshes the mirror list.
pub const REFLECTOR_TIMER: &str = "reflector.timer";

/// The search path the helper gives pacman, for the programs pacman itself starts.
pub const SEARCH_PATH: &str = "/usr/bin:/usr/sbin";

/// The longest package name pacman accepts, in bytes.
const MAX_NAME: usize = 255;

/// The longest locale name passed with [`LANG_FLAG`], in bytes.
const MAX_LOCALE: usize = 64;

/// What the helper is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Install these packages from the repositories.
    Install(Vec<String>),
    /// Bring the whole system up to date and install these packages in the same transaction.
    UpgradeInstall(Vec<String>),
    /// Bring the whole system up to date.
    Upgrade,
    /// Remove these packages with the dependencies only they needed and their saved settings.
    Remove(Vec<String>),
    /// Remove the orphans. The helper lists them itself; these are the ones qpac showed, and the
    /// request is refused with [`Refusal::Changed`] when the two lists differ.
    RemoveOrphans(Vec<String>),
    /// Mark these packages as installed only as dependencies.
    MarkDeps(Vec<String>),
    /// Mark these packages as installed on purpose.
    MarkExplicit(Vec<String>),
    /// Start and enable [`REFLECTOR_TIMER`] (`true`), or stop and disable it (`false`).
    Timer(bool),
    /// Take a snapshot before or after an update.
    Snapshot(Snapshot),
    /// Choose pacman's mirrors with reflector and save the choice for reflector's timer.
    Mirrors(Mirrors),
    /// Run later commands on a pseudo-terminal of this many columns and rows.
    Size {
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
}

/// Why a request was refused without running anything, or why the helper would not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The helper is not running as root.
    NotRoot,
    /// The request's first word is not one the helper knows.
    Unknown,
    /// A value is missing, extra or malformed.
    Values,
    /// A package name breaks pacman's naming rule.
    Name,
    /// The program could not be started, or the query the request depends on failed.
    Start,
    /// What the request was based on is no longer true: the orphans the helper found are not the
    /// ones qpac showed, because another transaction ran in between.
    Changed,
    /// reflector ended well but its list names no server; the mirror list was left as it was.
    NoMirrors,
    /// A file could not be written; nothing was replaced.
    File,
}

impl Refusal {
    const ALL: [Self; 8] = [
        Self::NotRoot,
        Self::Unknown,
        Self::Values,
        Self::Name,
        Self::Start,
        Self::Changed,
        Self::NoMirrors,
        Self::File,
    ];

    /// The word that names the refusal on the wire and in the language files.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::NotRoot => "not-root",
            Self::Unknown => "unknown",
            Self::Values => "values",
            Self::Name => "name",
            Self::Start => "start",
            Self::Changed => "changed",
            Self::NoMirrors => "no-mirrors",
            Self::File => "file",
        }
    }

    /// The refusal named `key`, when there is one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|refusal| refusal.key() == key)
    }
}

impl Request {
    /// Reads one request line, without its newline. Nothing is guessed: an unknown word, a
    /// missing or extra value, or a value that breaks its rule is refused.
    ///
    /// # Errors
    ///
    /// Returns why the line is not a request the helper carries out.
    pub fn parse(line: &str) -> Result<Self, Refusal> {
        let mut words = line.split(' ');
        let word = words.next().unwrap_or_default();
        let values: Vec<&str> = words.collect();
        match word {
            "install" => names(&values).map(Self::Install),
            "upgrade-install" => names(&values).map(Self::UpgradeInstall),
            "upgrade" => nothing(&values).map(|()| Self::Upgrade),
            "remove" => names(&values).map(Self::Remove),
            "remove-orphans" => names(&values).map(Self::RemoveOrphans),
            "mark-deps" => names(&values).map(Self::MarkDeps),
            "mark-explicit" => names(&values).map(Self::MarkExplicit),
            "timer" => match values.as_slice() {
                ["on", REFLECTOR_TIMER] => Ok(Self::Timer(true)),
                ["off", REFLECTOR_TIMER] => Ok(Self::Timer(false)),
                _ => Err(Refusal::Values),
            },
            "snapshot" => match values.as_slice() {
                ["pre", "snapper"] => Ok(Self::Snapshot(Snapshot::SnapperPre)),
                ["post", "snapper", number] => {
                    snapshot_number(number).map(|n| Self::Snapshot(Snapshot::SnapperPost(n)))
                }
                ["pre", "timeshift"] => Ok(Self::Snapshot(Snapshot::Timeshift)),
                _ => Err(Refusal::Values),
            },
            "mirrors" => Mirrors::from_values(&values).map(Self::Mirrors).ok_or(Refusal::Values),
            "size" => match values.as_slice() {
                [cols, rows] => Ok(Self::Size { cols: dimension(cols)?, rows: dimension(rows)? }),
                _ => Err(Refusal::Values),
            },
            _ => Err(Refusal::Unknown),
        }
    }

    /// The program, by its absolute path, and the fixed arguments that carry the request out;
    /// `None` for a request that runs nothing ([`Request::Size`]) or whose command depends on
    /// what the helper finds first ([`Request::RemoveOrphans`]) or that is followed by file work
    /// ([`Request::Mirrors`]).
    #[must_use]
    pub fn command(&self) -> Option<(&'static str, Vec<String>)> {
        let pacman = |args| Some((PACMAN_PATH, args));
        match self {
            Self::Install(names) => pacman(command::install(names)),
            Self::UpgradeInstall(names) => pacman(command::upgrade_install(names)),
            Self::Upgrade => pacman(command::upgrade()),
            Self::Remove(names) => pacman(command::remove(names)),
            Self::MarkDeps(names) => pacman(command::mark_deps(names)),
            Self::MarkExplicit(names) => pacman(command::mark_explicit(names)),
            Self::Timer(on) => {
                let verb = if *on { "enable" } else { "disable" };
                Some((SYSTEMCTL_PATH, [verb, "--now", "--", REFLECTOR_TIMER].map(str::to_owned).to_vec()))
            }
            Self::Snapshot(snapshot) => Some(snapshot.command()),
            Self::RemoveOrphans(_) | Self::Mirrors(_) | Self::Size { .. } => None,
        }
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Install(names) => write!(f, "install {}", names.join(" ")),
            Self::UpgradeInstall(names) => write!(f, "upgrade-install {}", names.join(" ")),
            Self::Upgrade => f.write_str("upgrade"),
            Self::Remove(names) => write!(f, "remove {}", names.join(" ")),
            Self::RemoveOrphans(names) => write!(f, "remove-orphans {}", names.join(" ")),
            Self::MarkDeps(names) => write!(f, "mark-deps {}", names.join(" ")),
            Self::MarkExplicit(names) => write!(f, "mark-explicit {}", names.join(" ")),
            Self::Timer(on) => write!(f, "timer {} {REFLECTOR_TIMER}", if *on { "on" } else { "off" }),
            Self::Snapshot(Snapshot::SnapperPre) => f.write_str("snapshot pre snapper"),
            Self::Snapshot(Snapshot::SnapperPost(number)) => write!(f, "snapshot post snapper {number}"),
            Self::Snapshot(Snapshot::Timeshift) => f.write_str("snapshot pre timeshift"),
            Self::Mirrors(mirrors) => write!(f, "mirrors {}", mirrors.values().join(" ")),
            Self::Size { cols, rows } => write!(f, "size {cols} {rows}"),
        }
    }
}

/// One line the helper writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// The helper is up and speaks this protocol version.
    Ready(u32),
    /// A line of the running command's output.
    Line(String),
    /// The command ended with this exit code, or `None` when a signal ended it.
    Done(Option<i32>),
    /// The request was refused and nothing ran.
    Refused(Refusal),
}

impl Response {
    /// Reads one response line, without its newline; `None` for anything else.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        if line == "line" {
            return Some(Self::Line(String::new()));
        }
        if let Some(text) = line.strip_prefix("line ") {
            return Some(Self::Line(text.to_owned()));
        }
        let (word, value) = line.split_once(' ')?;
        match word {
            "ready" => value.parse().ok().map(Self::Ready),
            "done" if value == "signal" => Some(Self::Done(None)),
            "done" => value.parse().ok().map(|code| Self::Done(Some(code))),
            "refused" => Refusal::from_key(value).map(Self::Refused),
            _ => None,
        }
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready(version) => write!(f, "ready {version}"),
            // A newline inside the text would end the line early and be read as a response.
            Self::Line(text) => write!(f, "line {}", text.replace('\n', " ")),
            Self::Done(Some(code)) => write!(f, "done {code}"),
            Self::Done(None) => f.write_str("done signal"),
            Self::Refused(refusal) => write!(f, "refused {}", refusal.key()),
        }
    }
}

/// Whether `name` follows pacman's rule for package names: only `a-z 0-9 @ . _ + -`, not
/// starting with `-` or `.`, at most 255 bytes.
#[must_use]
pub fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with(['-', '.'])
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"@._+-".contains(&b))
}

/// Whether `locale` looks like a locale name such as `tr_TR.UTF-8` or `C`: a letter first, then
/// letters, digits and `_ . @ -`, at most 64 bytes.
#[must_use]
pub fn is_locale(locale: &str) -> bool {
    locale.len() <= MAX_LOCALE
        && locale.starts_with(|c: char| c.is_ascii_alphabetic())
        && locale.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.@-".contains(&b))
}

/// The arguments after `sudo` that start the helper at `exe` without asking for a password;
/// `locale` is passed on only when it passes [`is_locale`], and the helper falls back to `C`.
#[must_use]
pub fn start_args(exe: &Path, locale: Option<&str>) -> Vec<String> {
    let mut args = vec!["-n".to_owned(), exe.to_string_lossy().into_owned(), FLAG.to_owned()];
    if let Some(locale) = locale.filter(|locale| is_locale(locale)) {
        args.extend([LANG_FLAG.to_owned(), locale.to_owned()]);
    }
    args
}

/// Reads the helper's own arguments, those after [`FLAG`]: nothing, or [`LANG_FLAG`] and a
/// locale. Returns the locale for pacman's shown output.
///
/// # Errors
///
/// Returns [`Refusal::Values`] for anything else, so a mistyped start runs nothing.
pub fn parse_options(args: &[String]) -> Result<String, Refusal> {
    match args {
        [] => Ok("C".to_owned()),
        [flag, locale] if flag == LANG_FLAG && is_locale(locale) => Ok(locale.clone()),
        _ => Err(Refusal::Values),
    }
}

/// The whole environment the helper sets for pacman on top of its own: a fixed search path, and
/// `locale` for both locale variables because the output is shown, never parsed.
#[must_use]
pub fn pacman_env(locale: &str) -> [(&'static str, &str); 3] {
    [("PATH", SEARCH_PATH), ("LANG", locale), ("LC_ALL", locale)]
}

/// At least one name, every one following [`is_package_name`].
fn names(values: &[&str]) -> Result<Vec<String>, Refusal> {
    if values.is_empty() {
        return Err(Refusal::Values);
    }
    if !values.iter().all(|name| is_package_name(name)) {
        return Err(Refusal::Name);
    }
    Ok(values.iter().map(|name| (*name).to_owned()).collect())
}

/// No value at all.
fn nothing(values: &[&str]) -> Result<(), Refusal> {
    if values.is_empty() { Ok(()) } else { Err(Refusal::Values) }
}

/// A snapshot number: digits only, without a leading zero, from 1 to `u32::MAX`.
fn snapshot_number(value: &str) -> Result<u32, Refusal> {
    if value.starts_with('0') || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Refusal::Values);
    }
    value.parse().map_err(|_| Refusal::Values)
}

/// A terminal dimension: digits only, from 1 to `u16::MAX`.
fn dimension(value: &str) -> Result<u16, Refusal> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Refusal::Values);
    }
    value.parse().ok().filter(|&cells| cells > 0).ok_or(Refusal::Values)
}

//! The line protocol between qpackages and its root helper.
//!
//! The helper is qpackages itself, started as root with [`FLAG`]. It reads one request per line
//! on its standard input and answers line by line on its standard output. Everything here is
//! plain text parsed by hand: the helper runs as root, so what it accepts is kept small enough
//! to read in one sitting, and every value is checked before anything runs.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::backup::Snapshot;
use crate::flatpak::{self, FLATPAK_PATH, is_app_id};
use crate::pacman::command;
use crate::reflector::Mirrors;
use crate::snap;

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

/// systemctl by its absolute path, for the two units the helper may switch.
pub const SYSTEMCTL_PATH: &str = "/usr/bin/systemctl";

/// One of the two units `timer` accepts: reflector's own timer, which refreshes the mirror
/// list. The other is [`snap::SOCKET_UNIT`].
pub const REFLECTOR_TIMER: &str = "reflector.timer";

/// The search path the helper gives pacman, for the programs pacman itself starts.
pub const SEARCH_PATH: &str = "/usr/bin:/usr/sbin";

/// The longest package name pacman accepts, in bytes.
const MAX_NAME: usize = 255;

/// The longest locale name passed with [`LANG_FLAG`], in bytes.
const MAX_LOCALE: usize = 64;

/// The longest path of a built package, in bytes: Linux's own limit on a path.
const MAX_PATH: usize = 4096;

/// The endings of the package files paru and yay build: makepkg's default compression and the
/// one it used before.
pub const BUILT_ENDINGS: [&str; 2] = [".pkg.tar.zst", ".pkg.tar.xz"];

/// Where paru and yay keep what they build, under the calling user's home folder. Only a file
/// under one of these is installed with [`Request::InstallBuilt`].
pub const BUILD_CACHES: [&str; 2] = [".cache/paru/clone", ".cache/yay"];

/// Where the background check downloads pending updates ahead, under the calling user's home
/// folder. Before a system update the helper copies what it finds here into pacman's own cache;
/// pacman then checks every copy's signature and checksum there, where the user cannot touch it
/// any more.
///
/// A fixed place under the home folder rather than the user's `XDG_CACHE_HOME`: the helper runs
/// as root with an environment of its own and finds the folder only through the password
/// database.
pub const DOWNLOADS: &str = ".cache/quvyta/packages/downloads";

/// pacman-conf by its absolute path, which says where pacman keeps its cache.
pub const PACMAN_CONF_PATH: &str = "/usr/bin/pacman-conf";

/// The endings of what `pacman -Sw` leaves in a cache: package files and their signatures.
const DOWNLOAD_ENDINGS: [&str; 4] = [".pkg.tar.zst", ".pkg.tar.xz", ".pkg.tar.zst.sig", ".pkg.tar.xz.sig"];

/// Whether `name` is a file name `pacman -Sw` writes: a package or its signature, as
/// `name-version-release-arch.pkg.tar.zst`, with no path in it and nothing hidden.
///
/// The helper copies only such names into pacman's cache, where pacman looks for exactly those.
#[must_use]
pub fn is_download_name(name: &str) -> bool {
    let Some(stem) = DOWNLOAD_ENDINGS.iter().find_map(|ending| name.strip_suffix(ending)) else { return false };
    // Name, version, release and architecture: at least three dashes in what is left.
    name.len() <= MAX_NAME
        && !name.starts_with(['.', '-'])
        && stem.matches('-').count() >= 3
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._+-:".contains(&b))
}

/// The variables that name the user who started the helper: pkexec sets the first, sudo the
/// second. Both are set by the program that grants root, not by the user, so the helper can
/// believe them.
pub const CALLER_VARIABLES: [&str; 2] = ["PKEXEC_UID", "SUDO_UID"];

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
    /// Install these package files, which paru or yay built as the user. Each is checked against
    /// the file rule where it is opened, so this carries only paths that pass
    /// [`is_built_path`].
    InstallBuilt(Vec<String>),
    /// Start and enable [`REFLECTOR_TIMER`] (`true`), or stop and disable it (`false`).
    Timer(bool),
    /// Take a snapshot before or after an update.
    Snapshot(Snapshot),
    /// Choose pacman's mirrors with reflector and save the choice for reflector's timer.
    Mirrors(Mirrors),
    /// Remove these Flatpak applications, installed for the whole system. Installing always
    /// happens for the user and needs no helper.
    FlatpakSystemRemove(Vec<String>),
    /// Have snapd install, remove or refresh these snaps. An empty list is only a refresh, and
    /// means every snap. snapd refuses an ordinary user all three, whether or not polkit is
    /// installed, so there is no path around the helper.
    Snap(snap::Job, Vec<String>),
    /// Start and enable [`snap::SOCKET_UNIT`] (`true`), or stop and disable it (`false`).
    ///
    /// On the wire this is a `timer` request, the way the design writes it, since the two units
    /// the helper may switch are switched with the same command. In code the two are apart
    /// because what the screen says about them has nothing in common.
    SnapdSocket(bool),
    /// Make [`snap::SNAP_LINK`] point at [`snap::SNAP_DIR`], which a snap with classic
    /// confinement insists on. Anything already at that path is left alone.
    SnapLink,
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
    /// A Flatpak application id breaks the id rule.
    FlatpakId,
    /// A built package file is not one the helper may install: not a regular file of the calling
    /// user's in their own build cache.
    Built,
    /// The request is not one the AUR build in progress was expected to make, so qpac did not
    /// pass it on.
    NotThisBuild,
    /// A snap name breaks snapd's naming rule.
    SnapName,
    /// Something other than the link to snapd's own folder is already at [`snap::SNAP_LINK`];
    /// the helper does not replace what it did not put there.
    SnapLink,
}

impl Refusal {
    /// Every refusal, for tables that name each one.
    pub const ALL: [Self; 13] = [
        Self::NotRoot,
        Self::Unknown,
        Self::Values,
        Self::Name,
        Self::Start,
        Self::Changed,
        Self::NoMirrors,
        Self::File,
        Self::FlatpakId,
        Self::Built,
        Self::NotThisBuild,
        Self::SnapName,
        Self::SnapLink,
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
            Self::FlatpakId => "flatpak-id",
            Self::Built => "built",
            Self::NotThisBuild => "not-this-build",
            Self::SnapName => "snap-name",
            Self::SnapLink => "snap-link",
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
            "install-built" => built_paths(&values).map(Self::InstallBuilt),
            "timer" => match values.as_slice() {
                ["on", REFLECTOR_TIMER] => Ok(Self::Timer(true)),
                ["off", REFLECTOR_TIMER] => Ok(Self::Timer(false)),
                ["on", snap::SOCKET_UNIT] => Ok(Self::SnapdSocket(true)),
                ["off", snap::SOCKET_UNIT] => Ok(Self::SnapdSocket(false)),
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
            "flatpak-system" => match values.split_first() {
                Some((&"remove", ids)) => app_ids(ids).map(Self::FlatpakSystemRemove),
                _ => Err(Refusal::Values),
            },
            "snap" => match values.split_first() {
                Some((word, names)) => match snap::Job::from_key(word) {
                    Some(job) => snap_names(job, names).map(|names| Self::Snap(job, names)),
                    None => Err(Refusal::Values),
                },
                None => Err(Refusal::Values),
            },
            "snap-link" => nothing(&values).map(|()| Self::SnapLink),
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
    /// what the helper finds first ([`Request::RemoveOrphans`], [`Request::InstallBuilt`]) or
    /// that is followed by file work ([`Request::Mirrors`]).
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
            Self::FlatpakSystemRemove(ids) => Some((FLATPAK_PATH, flatpak::system_uninstall_args(ids))),
            Self::SnapdSocket(on) => {
                let verb = if *on { "enable" } else { "disable" };
                Some((SYSTEMCTL_PATH, [verb, "--now", "--", snap::SOCKET_UNIT].map(str::to_owned).to_vec()))
            }
            Self::RemoveOrphans(_)
            | Self::InstallBuilt(_)
            | Self::Mirrors(_)
            | Self::Snap(_, _)
            | Self::SnapLink
            | Self::Size { .. } => None,
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
            Self::InstallBuilt(paths) => write!(f, "install-built {}", paths.join(" ")),
            Self::Timer(on) => write!(f, "timer {} {REFLECTOR_TIMER}", if *on { "on" } else { "off" }),
            Self::Snapshot(Snapshot::SnapperPre) => f.write_str("snapshot pre snapper"),
            Self::Snapshot(Snapshot::SnapperPost(number)) => write!(f, "snapshot post snapper {number}"),
            Self::Snapshot(Snapshot::Timeshift) => f.write_str("snapshot pre timeshift"),
            Self::Mirrors(mirrors) => write!(f, "mirrors {}", mirrors.values().join(" ")),
            Self::FlatpakSystemRemove(ids) => write!(f, "flatpak-system remove {}", ids.join(" ")),
            Self::Snap(job, names) if names.is_empty() => write!(f, "snap {}", job.key()),
            Self::Snap(job, names) => write!(f, "snap {} {}", job.key(), names.join(" ")),
            Self::SnapdSocket(on) => {
                write!(f, "timer {} {}", if *on { "on" } else { "off" }, snap::SOCKET_UNIT)
            }
            Self::SnapLink => f.write_str("snap-link"),
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
    /// snapd took the job on and numbered it. qpac follows the job's progress itself, reading
    /// `/v2/changes/<id>` as the user, while the helper waits for it to end.
    SnapChange(u32),
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
            "snap-change" => value.parse().ok().map(Self::SnapChange),
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
            Self::SnapChange(id) => write!(f, "snap-change {id}"),
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

/// Whether `path` may name a package paru or yay built, by its spelling alone: absolute, without
/// a `.` or `..` part or an empty one, ending in one of [`BUILT_ENDINGS`], at most 4096 bytes and
/// without control characters. Where the file lies and what it is are checked where it is opened.
#[must_use]
pub fn is_built_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else { return false };
    path.len() <= MAX_PATH
        && !path.chars().any(char::is_control)
        && BUILT_ENDINGS.iter().any(|ending| path.len() > ending.len() + 1 && path.ends_with(ending))
        && rest.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The build caches of the user whose home folder is `home`, as [`BUILD_CACHES`] names them.
#[must_use]
pub fn build_caches(home: &Path) -> Vec<PathBuf> {
    BUILD_CACHES.iter().map(|cache| home.join(cache)).collect()
}

/// Whether `path` lies inside one of the build caches of the user whose home folder is `home`,
/// by its spelling: below the cache folder, never the folder itself.
#[must_use]
pub fn in_build_cache(path: &Path, home: &Path) -> bool {
    !path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        && build_caches(home).iter().any(|cache| path.starts_with(cache) && path != cache)
}

/// The user who started the helper, from the first of [`CALLER_VARIABLES`] that `lookup` finds:
/// digits only, as pkexec and sudo write it.
#[must_use]
pub fn caller_uid(lookup: impl Fn(&str) -> Option<String>) -> Option<u32> {
    CALLER_VARIABLES.iter().find_map(|name| {
        let value = lookup(name)?;
        let digits = !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
        digits.then(|| value.parse().ok()).flatten()
    })
}

/// The home folder of user `uid` in `passwd`, the password database's text: the sixth field of
/// the first line whose third field is the id. Only an absolute path counts.
#[must_use]
pub fn passwd_home(passwd: &str, uid: u32) -> Option<PathBuf> {
    let wanted = uid.to_string();
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        match fields[..] {
            [_, _, id, _, _, home, _] if id == wanted && home.starts_with('/') => Some(PathBuf::from(home)),
            _ => None,
        }
    })
}

/// The login name of `uid` in the password database text `passwd`.
#[must_use]
pub fn passwd_name(passwd: &str, uid: u32) -> Option<String> {
    let wanted = uid.to_string();
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        match fields[..] {
            [name, _, id, _, _, _, _] if id == wanted && !name.is_empty() => Some(name.to_owned()),
            _ => None,
        }
    })
}

/// At least one path, every one following [`is_built_path`].
fn built_paths(values: &[&str]) -> Result<Vec<String>, Refusal> {
    if values.is_empty() {
        return Err(Refusal::Values);
    }
    if !values.iter().all(|path| is_built_path(path)) {
        return Err(Refusal::Built);
    }
    Ok(values.iter().map(|path| (*path).to_owned()).collect())
}

/// At least one Flatpak application id, every one following [`is_app_id`].
fn app_ids(values: &[&str]) -> Result<Vec<String>, Refusal> {
    if values.is_empty() {
        return Err(Refusal::Values);
    }
    if !values.iter().all(|id| is_app_id(id)) {
        return Err(Refusal::FlatpakId);
    }
    Ok(values.iter().map(|id| (*id).to_owned()).collect())
}

/// The names of a snap job: every one following [`snap::is_snap_name`]. Only a refresh may come
/// with none, and then it means every snap.
fn snap_names(job: snap::Job, values: &[&str]) -> Result<Vec<String>, Refusal> {
    if values.is_empty() {
        return if job.takes_all() { Ok(Vec::new()) } else { Err(Refusal::Values) };
    }
    if !values.iter().all(|name| snap::is_snap_name(name)) {
        return Err(Refusal::SnapName);
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

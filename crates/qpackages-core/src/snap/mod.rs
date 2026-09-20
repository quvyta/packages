//! What qpackages asks snapd to do, and how it reads the answers.
//!
//! Snap is two paths, not one. Reading — which snaps are installed, what a search finds, how far
//! a job has come — goes to snapd's socket as the user and comes back as JSON ([`api`]). Changing
//! the machine — installing, removing, refreshing — goes through the root helper, which runs the
//! `snap` program by its absolute path and checks every name with [`is_snap_name`] first. Every
//! rule here was measured against snapd 2.77.1 in a throwaway container; the recordings are under
//! `tests/fixtures/snap`.
//!
//! The `snap` program is never run without a socket to talk to: with none it retries for two
//! minutes before failing, which would look like a hang. [`api`] answers whether snapd is there
//! long before that.

pub mod api;

/// snapd's socket, the one both `snap` and qpackages talk to.
pub const SOCKET: &str = "/run/snapd.socket";

/// The unit that listens on [`SOCKET`]; the only snap unit the helper may switch.
pub const SOCKET_UNIT: &str = "snapd.socket";

/// snap by its absolute path, for the root helper, which never looks a program up.
pub const SNAP_PATH: &str = "/usr/bin/snap";

/// The program name, for the reads that run as the user.
pub const SNAP: &str = "snap";

/// Where snapd mounts the snaps it installed.
pub const SNAP_DIR: &str = "/var/lib/snapd/snap";

/// Where a snap with classic confinement insists on finding itself; on Arch this is a symbolic
/// link to [`SNAP_DIR`], which the package does not make.
pub const SNAP_LINK: &str = "/snap";

/// The shortest snap name snapd accepts, in bytes.
const MIN_NAME: usize = 2;

/// The longest snap name snapd accepts, in bytes.
const MAX_NAME: usize = 40;

/// Whether `name` is a snap name snapd would accept: lower-case ASCII letters, digits and `-`,
/// between 2 and 40 bytes, with at least one letter, not starting or ending with `-` and without
/// `--` inside.
///
/// The parallel-instance spelling `<name>_<key>` is left out on purpose: it needs an experimental
/// option turned on, so the helper has no reason to carry an underscore.
#[must_use]
pub fn is_snap_name(name: &str) -> bool {
    (MIN_NAME..=MAX_NAME).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && name.bytes().any(|b| b.is_ascii_lowercase())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// What the helper is asked to have snapd do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Install, confined the usual way.
    Install,
    /// Install a snap whose revision was published with classic confinement, which runs outside
    /// the sandbox. snapd refuses such a snap unless `--classic` says the user knows.
    InstallClassic,
    /// Remove, keeping the snapshot snapd takes of the application's data.
    Remove,
    /// Bring named snaps, or all of them, up to date.
    Refresh,
}

impl Job {
    /// The word the helper's request line uses.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::InstallClassic => "install-classic",
            Self::Remove => "remove",
            Self::Refresh => "refresh",
        }
    }

    /// The job named `key`, when there is one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Install, Self::InstallClassic, Self::Remove, Self::Refresh].into_iter().find(|job| job.key() == key)
    }

    /// Whether the job may come without names, meaning every snap. Only a refresh may.
    #[must_use]
    pub const fn takes_all(self) -> bool {
        matches!(self, Self::Refresh)
    }
}

/// The arguments that start `names` on their way, program name excluded.
///
/// `--no-wait` makes snapd answer with the job's number and let go, so the helper never holds a
/// pseudo-terminal open for minutes: the spinner and the redrawn progress bar snap prints on a
/// terminal are not a stream of lines anything could show. Progress is read from
/// [`api::change_path`] instead. `--` stands before the names so none can be read as an option.
#[must_use]
pub fn job_args(job: Job, names: &[String]) -> Vec<String> {
    let mut args = match job {
        Job::Install => vec!["install".to_owned()],
        Job::InstallClassic => vec!["install".to_owned(), "--classic".to_owned()],
        Job::Remove => vec!["remove".to_owned()],
        Job::Refresh => vec!["refresh".to_owned()],
    };
    args.push("--no-wait".to_owned());
    if !names.is_empty() {
        args.push("--".to_owned());
        args.extend_from_slice(names);
    }
    args
}

/// The arguments that wait for job `id` to end, program name excluded. On a pipe this prints
/// nothing and ends with the job's own code.
#[must_use]
pub fn watch_args(id: u32) -> Vec<String> {
    vec!["watch".to_owned(), id.to_string()]
}

/// The job number `--no-wait` printed, which is all it prints.
#[must_use]
pub fn change_id(stdout: &str) -> Option<u32> {
    let line = stdout.lines().map(str::trim).find(|line| !line.is_empty())?;
    line.parse().ok()
}

/// Something snapd said that the screen has to act on rather than only show.
///
/// Several of these come back with exit code 0, so the code alone never decides how a job went:
/// snapd calls "already installed" and "nothing to update" answers, not errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// The store has no snap by that name.
    NotFound,
    /// It is installed already. Exit code 0.
    AlreadyInstalled,
    /// It is not installed, so there was nothing to remove. Exit code 0.
    NotInstalled,
    /// Nothing to refresh. Exit code 0.
    NoUpdates,
    /// The revision has classic confinement; installing it needs `--classic`.
    ClassicNeeded,
    /// A classic snap cannot be installed while [`SNAP_LINK`] is missing.
    NoSnapLink,
    /// The name breaks snapd's own naming rule.
    InvalidName,
    /// snapd refused an ordinary user; the work belongs to the helper.
    Denied,
    /// A base snap another snap is running on cannot be removed.
    InUse,
    /// There is no socket to talk to: snapd is installed but not listening.
    NoSocket,
    /// The socket file is there but nothing is listening: snapd was stopped.
    SocketRefused,
}

/// The beginning of the warning `snap install` writes to its error output on a machine where
/// the session was not started again since snapd was installed. It is not a failure.
const PATH_WARNING: &str = "Warning: /var/lib/snapd/snap/bin was not found in your $PATH";

/// Whether `text` is only snapd's reminder that its folder is not on `PATH`. A job that printed
/// nothing else went well, whatever the warning looks like.
#[must_use]
pub fn is_path_warning(text: &str) -> bool {
    flatten(text).starts_with(PATH_WARNING)
}

/// What snapd's error output means, as far as the screen needs to know; `None` for anything
/// else, which is shown as it came.
///
/// snapd wraps its messages at about seventy columns and indents what it carried over, so the
/// text is joined back into one line before it is read.
#[must_use]
pub fn trouble(text: &str) -> Option<Trouble> {
    let text = flatten(text);
    // A missing snap is named in quotes: `snap "x" not found`. The bare words also appear in the
    // reminder that snapd's folder was not found in `$PATH`, which is no failure at all.
    let marks = [
        ("connect: no such file or directory", Trouble::NoSocket),
        ("connect: connection refused", Trouble::SocketRefused),
        ("access denied", Trouble::Denied),
        ("invalid snap name", Trouble::InvalidName),
        ("invalid instance name", Trouble::InvalidName),
        ("is already installed", Trouble::AlreadyInstalled),
        ("is not installed", Trouble::NotInstalled),
        ("has no updates available", Trouble::NoUpdates),
        ("All snaps up to date.", Trouble::NoUpdates),
        ("repeat the command including --classic", Trouble::ClassicNeeded),
        ("classic confinement requires snaps under /snap", Trouble::NoSnapLink),
        ("is not removable", Trouble::InUse),
        ("\" not found", Trouble::NotFound),
    ];
    marks.into_iter().find(|(mark, _)| text.contains(mark)).map(|(_, trouble)| trouble)
}

/// One line with every run of blank space, line ends among them, turned into a single space.
fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The arguments that list the snaps with an update waiting, program name excluded.
///
/// This one read runs as the user: snapd lets anyone ask, and it is the one place the `snap`
/// program is used rather than the socket, because its answer is short and needs no framing. It
/// is still only run once snapd is known to be listening ([`api`]); without that it would hang
/// for two minutes.
#[must_use]
pub fn refresh_list_args() -> Vec<String> {
    vec!["refresh".to_owned(), "--list".to_owned()]
}

/// One snap with a newer version waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// The snap's name.
    pub name: String,
    /// The version that would be installed.
    pub version: String,
}

/// Reads what [`refresh_list_args`] printed.
///
/// The answer is a table aligned with spaces, and a summary column further along can hold spaces
/// of its own, so only the first two columns are read: a snap's name never has a space in it
/// ([`is_snap_name`]) and neither does a version. The header line is skipped by its first word,
/// which is `Name` in every locale the program was measured in — snap does not translate it.
///
/// Nothing to update prints `All snaps up to date.` on the error stream and ends with 0, so an
/// empty answer here is the usual case, not a failure.
#[must_use]
pub fn parse_refresh_list(text: &str) -> Vec<Waiting> {
    text.lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let (name, version) = (columns.next()?, columns.next()?);
            (is_snap_name(name) && name != "Name")
                .then(|| Waiting { name: name.to_owned(), version: version.to_owned() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/snap").join(name);
        std::fs::read_to_string(path).expect("the recording is readable")
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_names_snapd_accepted_pass_the_rule() {
        for name in [
            "hello",
            "hello-world",
            "core22",
            "test-snapd-classic-confinement",
            "1a",
            "a1",
            "gnome-3-38-2004",
            &"a".repeat(MAX_NAME),
        ] {
            assert!(is_snap_name(name), "`{name}`");
        }
    }

    #[test]
    fn the_names_snapd_refused_fail_it() {
        for name in [
            "",
            "a",
            "123",
            "a--b",
            "-ab",
            "ab-",
            "Ab",
            "é",
            "hello_world",
            "hello world",
            "hello.world",
            "--classic",
            "hello/world",
            &"a".repeat(MAX_NAME + 1),
        ] {
            assert!(!is_snap_name(name), "`{name}`");
        }
    }

    #[test]
    fn the_argument_lists_are_the_measured_ones() {
        assert_eq!(job_args(Job::Install, &names(&["hello-world"])), ["install", "--no-wait", "--", "hello-world"]);
        assert_eq!(
            job_args(Job::InstallClassic, &names(&["code"])),
            ["install", "--classic", "--no-wait", "--", "code"]
        );
        assert_eq!(
            job_args(Job::Remove, &names(&["hello", "core22"])),
            ["remove", "--no-wait", "--", "hello", "core22"]
        );
        assert_eq!(job_args(Job::Refresh, &names(&["hello"])), ["refresh", "--no-wait", "--", "hello"]);
        assert_eq!(job_args(Job::Refresh, &[]), ["refresh", "--no-wait"], "every snap, with no name to separate");
        assert_eq!(watch_args(10), ["watch", "10"]);
    }

    #[test]
    fn every_job_is_known_by_its_word_and_only_a_refresh_may_come_bare() {
        for job in [Job::Install, Job::InstallClassic, Job::Remove, Job::Refresh] {
            assert_eq!(Job::from_key(job.key()), Some(job));
        }
        assert_eq!(Job::from_key("purge"), None);
        assert_eq!(Job::from_key(""), None);
        assert!(Job::Refresh.takes_all());
        assert!(!Job::Install.takes_all() && !Job::Remove.takes_all());
    }

    #[test]
    fn the_job_number_is_read_from_what_no_wait_printed() {
        assert_eq!(change_id(&fixture("install-no-wait.out")), Some(10));
        assert_eq!(change_id("\n  11 \n"), Some(11));
        assert_eq!(change_id(""), None);
        assert_eq!(change_id("hello-world 6.4 from Canonical** installed\n"), None, "a finished run says no number");
    }

    #[test]
    fn the_recorded_failures_are_told_apart() {
        for (name, expected) in [
            ("install-missing.err", Trouble::NotFound),
            ("install-already.err", Trouble::AlreadyInstalled),
            ("install-bad-name.err", Trouble::InvalidName),
            ("install-dash-name.err", Trouble::InvalidName),
            ("install-classic-needed.err", Trouble::ClassicNeeded),
            ("install-classic-no-snap-link.err", Trouble::NoSnapLink),
            ("remove-missing.err", Trouble::NotInstalled),
            ("remove-base-in-use.err", Trouble::InUse),
            ("refresh-one-none.err", Trouble::NoUpdates),
            ("refresh-all-none.err", Trouble::NoUpdates),
            ("refresh-list-none.err", Trouble::NoUpdates),
            ("user-install-denied.err", Trouble::Denied),
            ("list-no-socket.err", Trouble::NoSocket),
            ("find-no-socket.err", Trouble::NoSocket),
            ("list-socket-refused.err", Trouble::SocketRefused),
        ] {
            assert_eq!(trouble(&fixture(name)), Some(expected), "{name}");
        }
    }

    #[test]
    fn a_wrapped_message_reads_as_one_line() {
        let wrapped = fixture("install-classic-needed.err");
        assert!(wrapped.lines().count() > 2, "snapd wrapped this one");
        assert_eq!(trouble(&wrapped), Some(Trouble::ClassicNeeded), "the mark spans two lines");
    }

    #[test]
    fn the_path_reminder_is_no_failure() {
        let warning = fixture("install-first.err");
        assert!(is_path_warning(&warning));
        assert_eq!(trouble(&warning), None, "nothing to act on");
        assert!(!is_path_warning(&fixture("install-missing.err")));
        assert!(!is_path_warning(""));
        // The reminder says a folder was not found; a missing snap is named in quotes.
        assert_eq!(trouble("error: snap \"x\" not found"), Some(Trouble::NotFound));
    }

    #[test]
    fn what_went_well_says_nothing_to_act_on() {
        for name in ["install-first.out", "install-classic.out", "remove.out", "remove-two.out", "refresh-one.out"] {
            assert_eq!(trouble(&fixture(name)), None, "{name}");
        }
        assert_eq!(trouble(""), None);
    }

    #[test]
    fn the_snaps_with_an_update_waiting_are_read_from_the_recorded_table() {
        assert_eq!(refresh_list_args(), ["refresh", "--list"]);
        let waiting = parse_refresh_list(&fixture("refresh-list.out"));
        assert_eq!(waiting, [Waiting { name: String::from("hello"), version: String::from("2.10") }]);
        // Nothing to update goes to the error stream, so standard output is empty.
        assert_eq!(parse_refresh_list(&fixture("refresh-list-none.err")), [], "and that line is no snap");
        assert_eq!(parse_refresh_list(""), []);
        assert_eq!(parse_refresh_list("Name   Version\n"), [], "the header is not a snap");
        let mixed = "Name  Version\nhello  2.10\nBad_Name  1\nhello-world  6.4\n";
        let read = parse_refresh_list(mixed);
        let names: Vec<&str> = read.iter().map(|one| one.name.as_str()).collect();
        assert_eq!(names, ["hello", "hello-world"], "a line that is no snap name is left out");
    }
}

//! Talking to snapd on its socket, as the user.
//!
//! This is the whole reading side of Snap: which snaps are installed, what the store has, and how
//! far a job has come. None of it needs a permission — snapd answers an ordinary user, even about a
//! job root started — so nothing here goes near the helper.
//!
//! The `snap` program is never run to find out whether snapd is there. Without a socket it retries
//! for two minutes before giving up, and `snap version` takes twenty-five seconds to say the same,
//! so a missing snapd would look like a frozen screen. Connecting to the socket answers at once
//! instead: no file is "snapd is not switched on", a refused connection is "snapd was stopped", and
//! a connection that opens is snapd itself, which is then asked its version.
//!
//! The protocol is plain HTTP/1.0 written by hand (see [`api`]), so this needs no HTTP crate: a
//! request of four lines out, and everything up to the close read back in.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use qpackages_core::snap::api::{self, Change, Snap, SystemInfo};
use qpackages_core::sources::{Availability, Sources};

use crate::store::Failure;

/// How long a read from the socket may take before it is given up. snapd answers a list or a
/// search in milliseconds; this is only there so a snapd that has wedged cannot hold up the work
/// it runs in.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The largest answer read from the socket, in bytes. A search for a common word measured 46 kB
/// and the whole category list a few hundred; the limit is what keeps a runaway answer from
/// filling memory.
const MAX_ANSWER: usize = 8 << 20;

/// What snapd is on this machine, as the settings row and Discover need to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// snapd is not installed. Not an error: the source shows inactive and offers to install it.
    NotInstalled,
    /// snapd is installed but its socket was never switched on, so there is nothing to talk to.
    SocketMissing,
    /// The socket is there but nothing is listening: snapd was stopped.
    NotListening,
    /// snapd answered, and this is what it says it is.
    Ready(SystemInfo),
}

impl State {
    /// Whether snaps can be searched for and installed now. Discover offers Snap only then.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// Whether switching [`qpackages_core::snap::SOCKET_UNIT`] on is what this state needs. Both
    /// a socket that was never switched on and a snapd that was stopped are fixed by starting
    /// that unit.
    #[must_use]
    pub const fn wants_socket(&self) -> bool {
        matches!(self, Self::SocketMissing | Self::NotListening)
    }

    /// The language key of the row's own wording.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Self::NotInstalled => "snap.state.not-installed",
            Self::SocketMissing => "snap.state.socket-off",
            Self::NotListening => "snap.state.not-answering",
            Self::Ready(_) => "snap.state.ready",
        }
    }
}

/// snapd as this machine has it: where its socket is, and whether its program is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapd {
    /// The socket to talk to, [`qpackages_core::snap::SOCKET`] on a real machine.
    socket: PathBuf,
    /// Whether the `snap` program is installed. Without it snapd is not installed either.
    installed: bool,
}

impl Snapd {
    /// snapd on the socket at `socket`, `installed` saying whether the `snap` program is there.
    #[must_use]
    pub fn new(installed: bool, socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into(), installed }
    }

    /// snapd as `sources` found it, on the socket at `socket`.
    #[must_use]
    pub fn found(sources: &Sources, socket: impl Into<PathBuf>) -> Self {
        Self::new(matches!(sources.snap, Availability::Ready { .. }), socket)
    }

    /// Where the socket is, for the tests that answer on one of their own.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// What snapd is now. Blocks for as long as one connection and one short answer take.
    #[must_use]
    pub fn state(&self) -> State {
        if !self.installed {
            return State::NotInstalled;
        }
        match UnixStream::connect(&self.socket) {
            Ok(stream) => {
                match self.read(stream, api::SYSTEM_INFO).and_then(|body| ask_json(&body, api::parse_system_info)) {
                    Ok(info) => State::Ready(info),
                    // The socket opened and then said nothing usable: snapd is there but not working.
                    Err(_) => State::NotListening,
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::SocketMissing,
            Err(_) => State::NotListening,
        }
    }

    /// The installed snaps, the bases they run on among them.
    ///
    /// # Errors
    ///
    /// Returns why snapd could not be asked or its answer not read.
    pub fn installed(&self) -> Result<Vec<Snap>, Failure> {
        self.snaps(api::SNAPS)
    }

    /// The store's snaps matching `term`.
    ///
    /// # Errors
    ///
    /// Returns why snapd could not be asked or its answer not read.
    pub fn search(&self, term: &str) -> Result<Vec<Snap>, Failure> {
        self.snaps(&api::find_path(term))
    }

    /// What the store says about the snap named `name`; `None` when it has none.
    ///
    /// # Errors
    ///
    /// Returns why snapd could not be asked or its answer not read.
    pub fn about(&self, name: &str) -> Result<Option<Snap>, Failure> {
        Ok(self.snaps(&api::name_path(name))?.into_iter().next())
    }

    /// How far job `id` has come.
    ///
    /// # Errors
    ///
    /// Returns why snapd could not be asked or its answer not read.
    pub fn change(&self, id: u32) -> Result<Change, Failure> {
        let body = self.ask(&api::change_path(id))?;
        ask_json(&body, api::parse_change)
    }

    /// The snaps in the answer to `path`; a record that cannot be read is left out.
    fn snaps(&self, path: &str) -> Result<Vec<Snap>, Failure> {
        let body = self.ask(path)?;
        ask_json(&body, |text| api::parse_snaps(text).map(|(snaps, _)| snaps))
    }

    /// Writes the request for `path` and reads the whole answer's body.
    fn ask(&self, path: &str) -> Result<String, Failure> {
        let stream = UnixStream::connect(&self.socket).map_err(|_| Failure::Unreachable)?;
        self.read(stream, path)
    }

    /// Writes the request for `path` on `stream` and reads its body back.
    fn read(&self, mut stream: UnixStream, path: &str) -> Result<String, Failure> {
        let deadline = |result: std::io::Result<()>| result.map_err(|_| Failure::Unreachable);
        deadline(stream.set_read_timeout(Some(TIMEOUT)))?;
        deadline(stream.set_write_timeout(Some(TIMEOUT)))?;
        stream.write_all(api::request(path).as_bytes()).map_err(|_| Failure::Unreachable)?;
        stream.flush().map_err(|_| Failure::Unreachable)?;
        let mut bytes = Vec::new();
        // The request asked for HTTP/1.0 and a closed connection, so the body ends at the close.
        Read::take(stream, u64::try_from(MAX_ANSWER).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Unreachable)?;
        let answer = String::from_utf8(bytes).map_err(|_| Failure::Unreadable)?;
        api::body(&answer).map(str::to_owned).map_err(|_| Failure::Unreadable)
    }
}

/// Reads `body` with `parse`, turning anything it reports into [`Failure::Unreadable`]: a broken
/// answer is a source that said nothing, not a reason to fail the screen.
fn ask_json<T, E>(body: &str, parse: impl Fn(&str) -> Result<T, E>) -> Result<T, Failure> {
    parse(body).map_err(|_| Failure::Unreadable)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use qpackages_core::snap::api;
    use qpackages_core::sources::{AurPreference, detect};

    use super::*;
    use crate::testing::{FakeSnapd, Scratch};

    /// The recordings of what snapd answered in the container.
    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/snap").join(name);
        std::fs::read_to_string(path).expect("the recording is readable")
    }

    /// A machine where `snap` is installed.
    fn with_snap(program: &str) -> Option<PathBuf> {
        ["pacman", "snap"].contains(&program).then(|| Path::new("/usr/bin").join(program))
    }

    /// A machine without it.
    fn without_snap(program: &str) -> Option<PathBuf> {
        (program == "pacman").then(|| Path::new("/usr/bin").join(program))
    }

    fn snapd(socket: &Path, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> Snapd {
        Snapd::found(&detect(AurPreference::Auto, lookup), socket)
    }

    #[test]
    fn without_the_program_snapd_is_not_installed_and_nothing_is_connected_to() {
        let scratch = Scratch::new("snap-missing", &[]);
        let snapd = snapd(&scratch.root().join("snapd.socket"), &without_snap);
        assert_eq!(snapd.state(), State::NotInstalled);
        assert!(!snapd.state().is_ready());
        assert!(!snapd.state().wants_socket(), "installing snapd comes first, not switching a socket on");
    }

    #[test]
    fn with_the_program_but_no_socket_file_the_socket_is_only_switched_off() {
        let scratch = Scratch::new("snap-no-socket", &[]);
        let snapd = snapd(&scratch.root().join("snapd.socket"), &with_snap);
        assert_eq!(snapd.state(), State::SocketMissing, "the package leaves snapd.socket disabled");
        assert!(snapd.state().wants_socket());
        assert_eq!(snapd.installed(), Err(Failure::Unreachable), "and nothing can be read");
    }

    #[test]
    fn a_socket_nothing_listens_on_says_snapd_was_stopped() {
        let scratch = Scratch::new("snap-refused", &[]);
        let socket = scratch.root().join("snapd.socket");
        // A plain file where the socket belongs is refused the way a stopped snapd's socket is.
        std::fs::write(&socket, "").expect("the scratch folder takes a file");
        let snapd = snapd(&socket, &with_snap);
        assert_eq!(snapd.state(), State::NotListening);
        assert!(snapd.state().wants_socket());
    }

    #[test]
    fn a_listening_snapd_answers_with_its_version() {
        let scratch = Scratch::new("snap-ready", &[]);
        let socket = scratch.root().join("snapd.socket");
        let fake = FakeSnapd::start(&socket);
        fake.answer(api::SYSTEM_INFO, &fixture("api-system-info.json"));
        let state = snapd(&socket, &with_snap).state();
        assert_eq!(state, State::Ready(SystemInfo { version: String::from("2.77.1-1"), series: String::from("16") }));
        assert!(state.is_ready() && !state.wants_socket());
        assert_eq!(fake.asked(), [api::SYSTEM_INFO]);
    }

    #[test]
    fn a_socket_that_answers_nothing_usable_is_a_snapd_that_is_not_working() {
        let scratch = Scratch::new("snap-mute", &[]);
        let socket = scratch.root().join("snapd.socket");
        let _fake = FakeSnapd::start(&socket);
        assert_eq!(snapd(&socket, &with_snap).state(), State::NotListening, "listening is not answering");
    }

    #[test]
    fn the_installed_snaps_and_a_search_are_read_from_the_socket() {
        let scratch = Scratch::new("snap-read", &[]);
        let socket = scratch.root().join("snapd.socket");
        let fake = FakeSnapd::start(&socket);
        fake.answer(api::SNAPS, &fixture("api-snaps.json"));
        fake.answer(&api::find_path("hello world"), &fixture("api-find-q.json"));
        let snapd = snapd(&socket, &with_snap);
        let installed = snapd.installed().expect("the recording reads");
        assert_eq!(installed.len(), 7);
        assert!(installed.iter().any(|snap| snap.name == "hello-world"));
        let found = snapd.search("hello world").expect("the recording reads");
        assert_eq!(found.len(), 17);
        assert_eq!(fake.asked(), [api::SNAPS.to_owned(), api::find_path("hello world")]);
    }

    #[test]
    fn one_snaps_record_and_a_jobs_progress_are_read_from_the_socket() {
        let scratch = Scratch::new("snap-one", &[]);
        let socket = scratch.root().join("snapd.socket");
        let fake = FakeSnapd::start(&socket);
        fake.answer(&api::name_path("hello-world"), &fixture("api-find-name.json"));
        fake.answer(&api::change_path(11), &fixture("api-change-doing.json"));
        let snapd = snapd(&socket, &with_snap);
        let about = snapd.about("hello-world").expect("the recording reads").expect("the store has it");
        assert_eq!(about.name, "hello-world");
        let change = snapd.change(11).expect("the recording reads");
        assert_eq!(change.status, "Doing");
        assert_eq!(change.running().map(|task| task.kind.as_str()), Some("download-snap"));
    }

    #[test]
    fn an_answer_that_is_not_json_is_unreadable_rather_than_unreachable() {
        let scratch = Scratch::new("snap-junk", &[]);
        let socket = scratch.root().join("snapd.socket");
        let fake = FakeSnapd::start(&socket);
        fake.answer(api::SNAPS, "not json at all");
        assert_eq!(snapd(&socket, &with_snap).installed(), Err(Failure::Unreadable));
    }

    #[test]
    fn a_store_without_the_name_answers_with_nothing_rather_than_a_failure() {
        let scratch = Scratch::new("snap-none", &[]);
        let socket = scratch.root().join("snapd.socket");
        let fake = FakeSnapd::start(&socket);
        fake.answer(&api::name_path("no-such-snap"), "{\"result\":[]}");
        assert_eq!(snapd(&socket, &with_snap).about("no-such-snap").expect("an empty list reads"), None);
    }
}

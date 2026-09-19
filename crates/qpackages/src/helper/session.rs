//! The helper's user side: the session that owns the root helper for as long as qpac runs.
//!
//! The helper is started once, either through pkexec while the terminal is handed over (see
//! [`super::pkexec`]), or after sudo has asked for the password on the real terminal, with
//! `sudo -n` so it never asks again. From then on every transaction goes to it without a prompt.
//! It ends when qpac quits, when the user lets it go, or when it dies; the next transaction then
//! starts a new one. Its only line to qpac is two pipes that no other program can reach.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use qframe::runtime::ProcessOutcome;
use qpackages_core::helper::{Refusal, Request, Response, VERSION, start_args};
use qpackages_core::pacman::command::SUDO;

/// How often a wait for the helper looks up to see whether it should stop waiting.
const POLL: Duration = Duration::from_millis(50);

/// How long a starting helper has to say it is ready. `sudo -n` never prompts, so only a stuck
/// start takes this long.
const READY_WAIT: Duration = Duration::from_secs(20);

/// How a wait for the helper passes the time and learns it was given up.
pub trait Wait {
    /// Whether the wait was given up.
    fn given_up(&self) -> bool;

    /// Lets about `duration` pass. Returns `false` once the wait was given up.
    fn pause(&self, duration: Duration) -> bool;
}

/// A closure that says whether the wait was given up; a pause sleeps on this thread.
impl<F: Fn() -> bool> Wait for F {
    fn given_up(&self) -> bool {
        self()
    }

    fn pause(&self, duration: Duration) -> bool {
        thread::sleep(duration);
        !self()
    }
}

/// Starts a helper and connects to it.
pub type Start = dyn Fn() -> io::Result<Connection> + Send + Sync;

/// Where a helper runs, as far as the connection needs to know.
pub trait Host: Send {
    /// Whether the helper is still running.
    fn is_alive(&mut self) -> bool;

    /// What the helper or the program that started it said on its way out, once it has gone.
    fn last_words(&mut self) -> String;
}

/// The two pipes to a running helper and the helper behind them.
///
/// Dropping it closes the helper's input first, which is how the helper learns to exit once its
/// job in hand is done.
pub struct Connection {
    requests: Box<dyn Write + Send>,
    responses: Receiver<io::Result<String>>,
    host: Box<dyn Host>,
    /// Whether the responses are handed over by the application's own loop rather than read by
    /// a thread of the connection's: a wait for them then pauses the way its caller pauses, so
    /// the loop that delivers them is never held up by it.
    handed: bool,
}

impl Connection {
    /// A connection that writes requests to `requests` and reads responses from `responses` on
    /// a thread of its own, so a wait can be given up without the helper's cooperation.
    #[must_use]
    pub fn new(requests: Box<dyn Write + Send>, responses: impl Read + Send + 'static, host: Box<dyn Host>) -> Self {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || read_responses(responses, &sender));
        Self { requests, responses: receiver, host, handed: false }
    }

    /// A connection whose responses someone else reads and hands over line by line, without
    /// their line endings: the helper's output ends when every sender of `responses` is gone.
    #[must_use]
    pub fn from_lines(
        requests: Box<dyn Write + Send>,
        responses: Receiver<io::Result<String>>,
        host: Box<dyn Host>,
    ) -> Self {
        Self { requests, responses, host, handed: true }
    }

    /// Sends one request.
    fn send(&mut self, request: &Request) -> io::Result<()> {
        writeln!(self.requests, "{request}")?;
        self.requests.flush()
    }

    /// Waits for the next response; `None` once the wait was given up.
    fn receive(&mut self, wait: &dyn Wait) -> io::Result<Option<Response>> {
        let block = if self.handed { Duration::ZERO } else { POLL };
        loop {
            match self.responses.recv_timeout(block) {
                Ok(Ok(line)) => {
                    return Response::parse(&line)
                        .map(Some)
                        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, line));
                }
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => {
                    let over = if self.handed { !wait.pause(POLL) } else { wait.given_up() };
                    if over {
                        return Ok(None);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, self.host.last_words()));
                }
            }
        }
    }
}

/// Hands every line of `stream` to `sender` without its line ending, until the stream ends.
fn read_responses(stream: impl Read, sender: &Sender<io::Result<String>>) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => return,
            Ok(_) => {
                bytes.pop_if(|byte| *byte == b'\n');
                bytes.pop_if(|byte| *byte == b'\r');
                if sender.send(Ok(String::from_utf8_lossy(&bytes).into_owned())).is_err() {
                    return;
                }
            }
            Err(error) => {
                // The receiver may be gone already; then nobody is left to tell.
                let _ = sender.send(Err(error));
                return;
            }
        }
    }
}

/// Why a helper did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartFailure {
    /// The helper started and refused to serve.
    Refused(Refusal),
    /// The helper said nothing in time.
    Silent,
    /// The helper could not be started or ended at once; what it or sudo said, which may be
    /// empty.
    Failed(String),
}

/// How a request sent to the helper ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// pacman ran and ended like this; [`ProcessOutcome::Cancelled`] when the wait was given up.
    Finished(ProcessOutcome),
    /// The helper refused the request and ran nothing.
    Refused(Refusal),
    /// The helper was gone or stopped making sense; what was known about it, which may be empty.
    Lost(String),
}

/// The helper of this run, if one is up, shared between the screen and the background work
/// that talks to it.
#[derive(Clone)]
pub struct Session {
    start: Arc<Start>,
    connection: Arc<Mutex<Option<Connection>>>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    /// A session with no helper yet, that starts one with `start` when asked.
    #[must_use]
    pub fn new(start: Arc<Start>) -> Self {
        Self { start, connection: Arc::new(Mutex::new(None)) }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Connection>> {
        self.connection.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a helper is up and idle. A helper found dead is let go here, so the next
    /// transaction starts a new one.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        let mut slot = self.lock();
        if slot.as_mut().is_some_and(|connection| !connection.host.is_alive()) {
            *slot = None;
        }
        slot.is_some()
    }

    /// Lets the helper go: its input closes and it exits.
    pub fn end(&self) {
        let connection = self.lock().take();
        drop(connection);
    }

    /// Starts a helper and waits until it says it is ready. Blocks; runs in the background.
    ///
    /// # Errors
    ///
    /// Returns why the helper is not ready; then no helper is kept.
    pub fn start(&self) -> Result<(), StartFailure> {
        self.end();
        let mut connection = (self.start)().map_err(|error| StartFailure::Failed(error.to_string()))?;
        let deadline = Instant::now() + READY_WAIT;
        match connection.receive(&|| Instant::now() >= deadline) {
            Ok(Some(first)) => self.keep(connection, first),
            Ok(None) => Err(StartFailure::Silent),
            Err(error) => Err(StartFailure::Failed(error.to_string())),
        }
    }

    /// Takes over a helper someone else started, whose first line was `first_line`: it is kept
    /// when that line says it is ready, as [`Session::start`] would.
    ///
    /// # Errors
    ///
    /// Returns why the helper is not ready; then it is let go.
    pub fn adopt(&self, connection: Connection, first_line: &str) -> Result<(), StartFailure> {
        self.end();
        match Response::parse(first_line) {
            Some(first) => self.keep(connection, first),
            None => Err(StartFailure::Failed(first_line.to_owned())),
        }
    }

    /// Keeps `connection` when its helper's first response says it is ready.
    fn keep(&self, connection: Connection, first: Response) -> Result<(), StartFailure> {
        match first {
            Response::Ready(VERSION) => {
                *self.lock() = Some(connection);
                Ok(())
            }
            Response::Refused(refusal) => Err(StartFailure::Refused(refusal)),
            other => Err(StartFailure::Failed(other.to_string())),
        }
    }

    /// Sends `request` to run on a `size` pseudo-terminal and hands every line of its output to
    /// `on_line` until it ends. Blocks; runs in the background.
    ///
    /// When `wait` is given up, the helper is let go: it runs pacman to its end, since stopping
    /// pacman halfway can break its database, then exits. A helper that is gone or answers
    /// something else is let go as well.
    pub fn run(
        &self,
        request: &Request,
        (cols, rows): (u16, u16),
        wait: &dyn Wait,
        on_line: &mut dyn FnMut(String),
    ) -> Outcome {
        // Taken out for the whole run, so the screen never waits on the lock while pacman works.
        let Some(mut connection) = self.lock().take() else {
            return Outcome::Lost(String::new());
        };
        if let Err(error) = connection.send(&Request::Size { cols, rows }).and_then(|()| connection.send(request)) {
            return Outcome::Lost(error.to_string());
        }
        loop {
            let ended = match connection.receive(wait) {
                Ok(Some(Response::Line(text))) => {
                    on_line(text);
                    continue;
                }
                Ok(Some(Response::Done(code))) => Outcome::Finished(ProcessOutcome::Finished { code }),
                Ok(Some(Response::Refused(refusal))) => Outcome::Refused(refusal),
                Ok(Some(Response::Ready(version))) => return Outcome::Lost(Response::Ready(version).to_string()),
                Ok(None) => return Outcome::Finished(ProcessOutcome::Cancelled),
                Err(error) => return Outcome::Lost(error.to_string()),
            };
            *self.lock() = Some(connection);
            return ended;
        }
    }
}

/// The helper as a child process: `sudo -n <this program> --privileged-helper`.
struct Sudo {
    /// The child, until it is handed to a thread that waits for it.
    child: Option<Child>,
    /// Collects what sudo and the helper write to their error stream.
    stderr: Option<JoinHandle<String>>,
}

impl Host for Sudo {
    fn is_alive(&mut self) -> bool {
        self.child.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    fn last_words(&mut self) -> String {
        if let Some(child) = self.child.as_mut() {
            // Its output has ended, so it is on its way out; the wait is short.
            let _ = child.wait();
        }
        let text = self.stderr.take().and_then(|thread| thread.join().ok()).unwrap_or_default();
        text.trim().to_owned()
    }
}

impl Drop for Sudo {
    /// Its input is already closed; a helper still finishing a job is waited for on a thread of
    /// its own, so quitting never waits for pacman and no finished child lingers.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            thread::spawn(move || child.wait());
        }
    }
}

/// Starts the helper as root through sudo, without a prompt: sudo's ticket was warmed on the
/// real terminal just before.
///
/// The helper runs in a process group of its own, so a signal the terminal sends to qpac, such
/// as the hangup when its window closes, does not stop pacman halfway; the closed pipe tells it
/// to exit once its job is done.
///
/// # Errors
///
/// Returns the error of a program that could not be started.
pub fn sudo() -> io::Result<Connection> {
    let exe = std::env::current_exe()?;
    let mut child = Command::new(SUDO)
        .args(start_args(&exe, locale().as_deref()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let (Some(stdin), Some(stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(io::Error::other("the helper's pipes were not opened"));
    };
    let stderr = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    Ok(Connection::new(Box::new(stdin), stdout, Box::new(Sudo { child: Some(child), stderr: Some(stderr) })))
}

/// The language the user reads messages in, handed to the helper so pacman's shown output is in
/// it: sudo and pkexec both give the helper a bare environment.
#[must_use]
pub fn locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

/// A helper that runs the root side's loop on a thread, with the recorded runner, for tests that
/// must exercise the real protocol and never reach sudo or pacman.
#[cfg(test)]
pub use in_process::InProcess;

#[cfg(test)]
mod in_process {
    use std::io::{self, BufReader, PipeWriter, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread::{self, JoinHandle};

    use super::{Connection, Host, Start};
    use crate::helper::root;
    use crate::runner::Recorded;

    /// The input of one helper, shared with [`InProcess`] so a test can close it.
    type Input = Arc<Mutex<Option<PipeWriter>>>;

    /// Starts helpers on threads and can make them die.
    pub struct InProcess {
        runner: Arc<Recorded>,
        uid: u32,
        inputs: Mutex<Vec<Input>>,
        threads: Mutex<Vec<JoinHandle<()>>>,
        starts: AtomicUsize,
    }

    impl InProcess {
        /// Helpers that run as user `uid` and play pacman from `runner`.
        pub fn new(runner: &Arc<Recorded>, uid: u32) -> Arc<Self> {
            Arc::new(Self {
                runner: Arc::clone(runner),
                uid,
                inputs: Mutex::new(Vec::new()),
                threads: Mutex::new(Vec::new()),
                starts: AtomicUsize::new(0),
            })
        }

        /// The session's way of starting one of these helpers.
        pub fn start_fn(self: &Arc<Self>) -> Arc<Start> {
            let launcher = Arc::clone(self);
            Arc::new(move || launcher.start())
        }

        /// How many helpers were started.
        pub fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }

        /// Makes every helper started so far die the way a killed process looks from outside:
        /// its pipes close and it is no longer running. Returns once all of them have ended.
        pub fn kill(&self) {
            for input in self.inputs.lock().unwrap_or_else(PoisonError::into_inner).drain(..) {
                input.lock().unwrap_or_else(PoisonError::into_inner).take();
            }
            for thread in self.threads.lock().unwrap_or_else(PoisonError::into_inner).drain(..) {
                thread.join().expect("the helper thread ends cleanly");
            }
        }

        fn start(&self) -> io::Result<Connection> {
            let (request_reader, request_writer) = io::pipe()?;
            let (response_reader, response_writer) = io::pipe()?;
            let ended = Arc::new(AtomicBool::new(false));
            let (runner, uid, flag) = (Arc::clone(&self.runner), self.uid, Arc::clone(&ended));
            let thread = thread::spawn(move || {
                let input = BufReader::new(request_reader);
                // No request these helpers get writes a file; a root that does not exist makes
                // sure one never could.
                let nowhere = std::env::temp_dir().join("qpackages-in-process-helper-root");
                let _ = root::answer(&[], Some(uid), &nowhere, input, response_writer, runner.as_ref());
                flag.store(true, Ordering::SeqCst);
            });
            let input: Input = Arc::new(Mutex::new(Some(request_writer)));
            self.inputs.lock().unwrap_or_else(PoisonError::into_inner).push(Arc::clone(&input));
            self.threads.lock().unwrap_or_else(PoisonError::into_inner).push(thread);
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Connection::new(Box::new(Shared(input)), response_reader, Box::new(Thread(ended))))
        }
    }

    /// A helper's input that a test can close from outside.
    struct Shared(Input);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self.0.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
                Some(pipe) => pipe.write(buf),
                None => Err(io::ErrorKind::BrokenPipe.into()),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.0.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
                Some(pipe) => pipe.flush(),
                None => Err(io::ErrorKind::BrokenPipe.into()),
            }
        }
    }

    impl Drop for Shared {
        fn drop(&mut self) {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        }
    }

    /// A helper on a thread, alive until its loop returns.
    struct Thread(Arc<AtomicBool>);

    impl Host for Thread {
        fn is_alive(&mut self) -> bool {
            !self.0.load(Ordering::SeqCst)
        }

        fn last_words(&mut self) -> String {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Recorded;

    fn install(name: &str) -> Request {
        Request::Install(vec![name.to_owned()])
    }

    fn install_args(name: &str) -> Vec<String> {
        ["-S", "--needed", "--noconfirm", "--", name].map(str::to_owned).to_vec()
    }

    fn session(recorded: &Arc<Recorded>, uid: u32) -> (Session, Arc<InProcess>) {
        let launcher = InProcess::new(recorded, uid);
        (Session::new(launcher.start_fn()), launcher)
    }

    #[test]
    fn a_changed_orphan_list_comes_back_as_a_refusal_and_the_helper_stays() {
        let recorded = Arc::new(Recorded::default());
        recorded.answer("/usr/bin/pacman", &["-Qtdq"], "libfoo\nlibbar\n", 0);
        let (session, _launcher) = session(&recorded, 0);
        assert_eq!(session.start(), Ok(()));
        let request = Request::RemoveOrphans(vec!["libfoo".to_owned()]);
        let outcome = session.run(&request, (80, 24), &|| false, &mut |_| {});
        assert_eq!(outcome, Outcome::Refused(Refusal::Changed));
        assert!(session.is_alive(), "a refusal leaves the helper ready for the next request");
    }

    #[test]
    fn a_started_helper_carries_out_requests_one_after_another() {
        let recorded = Arc::new(Recorded::default());
        recorded.play(
            "/usr/bin/pacman",
            &install_args("cowsay"),
            &["one", "two"],
            ProcessOutcome::Finished { code: Some(0) },
        );
        let (session, launcher) = session(&recorded, 0);
        assert!(!session.is_alive());
        assert_eq!(session.start(), Ok(()));
        assert!(session.is_alive());
        let mut lines = Vec::new();
        let outcome = session.run(&install("cowsay"), (70, 9), &|| false, &mut |line| lines.push(line));
        assert_eq!(outcome, Outcome::Finished(ProcessOutcome::Finished { code: Some(0) }));
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(recorded.calls()[0].pty, Some((70, 9)), "the size goes before the request");
        let outcome = session.run(&install("nope"), (70, 9), &|| false, &mut |_| {});
        assert_eq!(outcome, Outcome::Refused(Refusal::Start), "the recorded runner has no pacman for it");
        assert!(session.is_alive(), "a refusal leaves the helper up");
        assert_eq!(launcher.starts(), 1);
        session.end();
        assert!(!session.is_alive());
    }

    #[test]
    fn a_helper_that_is_not_root_refuses_to_start_and_is_not_kept() {
        let recorded = Arc::new(Recorded::default());
        let (session, _) = session(&recorded, 1000);
        assert_eq!(session.start(), Err(StartFailure::Refused(Refusal::NotRoot)));
        assert!(!session.is_alive());
        assert_eq!(session.run(&install("cowsay"), (80, 24), &|| false, &mut |_| {}), Outcome::Lost(String::new()));
    }

    #[test]
    fn a_program_that_cannot_start_is_a_failure_not_a_panic() {
        let session = Session::new(Arc::new(|| Err(io::Error::new(io::ErrorKind::NotFound, "no sudo"))));
        assert_eq!(session.start(), Err(StartFailure::Failed("no sudo".to_owned())));
    }

    #[test]
    fn a_dead_helper_is_noticed_and_let_go() {
        let recorded = Arc::new(Recorded::default());
        let (session, launcher) = session(&recorded, 0);
        assert_eq!(session.start(), Ok(()));
        launcher.kill();
        assert!(!session.is_alive());
        assert_eq!(session.start(), Ok(()), "a new one starts");
        let clone = session.clone();
        launcher.kill();
        assert!(matches!(clone.run(&install("cowsay"), (80, 24), &|| false, &mut |_| {}), Outcome::Lost(_)));
        assert!(!session.is_alive(), "a lost helper is not kept");
        assert_eq!(launcher.starts(), 2);
    }

    /// A helper that is alive and has nothing more to say.
    struct Quiet;

    impl Host for Quiet {
        fn is_alive(&mut self) -> bool {
            true
        }

        fn last_words(&mut self) -> String {
            String::new()
        }
    }

    #[test]
    fn giving_up_the_wait_lets_the_helper_go() {
        let (reader, _writer) = io::pipe().expect("a pipe opens");
        let reader = Mutex::new(Some(reader));
        let session = Session::new(Arc::new(move || {
            let reader = reader.lock().unwrap_or_else(PoisonError::into_inner).take().expect("started once");
            Ok(Connection::new(Box::new(io::sink()), (&b"ready 1\n"[..]).chain(reader), Box::new(Quiet)))
        }));
        assert_eq!(session.start(), Ok(()));
        let outcome = session.run(&install("cowsay"), (80, 24), &|| true, &mut |_| {});
        assert_eq!(outcome, Outcome::Finished(ProcessOutcome::Cancelled));
        assert!(!session.is_alive(), "a helper nobody waits for is let go");
    }

    #[test]
    fn a_silent_or_strange_helper_is_not_kept() {
        let strange =
            Session::new(Arc::new(|| Ok(Connection::new(Box::new(io::sink()), &b"ready 2\n"[..], Box::new(Quiet)))));
        assert_eq!(strange.start(), Err(StartFailure::Failed("ready 2".to_owned())));
        let gone = Session::new(Arc::new(|| Ok(Connection::new(Box::new(io::sink()), io::empty(), Box::new(Quiet)))));
        assert_eq!(gone.start(), Err(StartFailure::Failed(String::new())));
        assert!(!strange.is_alive() && !gone.is_alive());
    }
}

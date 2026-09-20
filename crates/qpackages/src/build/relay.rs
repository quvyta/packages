//! The qpac side of an AUR build's pipes: reads each shim's request, passes on the ones the build
//! is expected to make to the helper, and writes the helper's answer back.
//!
//! The request pipe is held open for reading and writing for the whole build, so it never looks
//! ended between two shims and a shim can tell qpac is there. Each answer is written through its
//! own opening of the answer pipe; closing it tells the shim the answer is complete.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use qframe::runtime::ProcessOutcome;
use qpackages_core::build::Expected;
use qpackages_core::helper::{Refusal, Request, Response};
use rustix::fs::{Mode, OFlags, fcntl_getfl, fcntl_setfl, open};

use super::pipes::Pipes;
use crate::helper::session::{Outcome, Session};

/// The longest request line read, in bytes, as the helper's own limit.
const MAX_LINE: usize = 1 << 20;

/// What the relay needs to answer a build's requests.
pub struct Setup {
    /// The helper of this run, which carries the requests out.
    pub session: Session,
    /// What the build may ask for.
    pub expected: Expected,
    /// The pseudo-terminal pacman gets, in columns and rows.
    pub size: (u16, u16),
    /// What each refusal means in the user's words, said before the refusal itself so the build's
    /// output shows why a step did not run.
    pub refusals: Vec<(Refusal, String)>,
}

/// The relay of one build, running on a thread of its own until stopped.
pub struct Relay {
    stop: Arc<AtomicBool>,
    request: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    /// Starts relaying the requests that come through `pipes`. The request pipe is opened here,
    /// before the build starts, so the first shim finds it read.
    ///
    /// # Errors
    ///
    /// Returns the error of a pipe that could not be opened.
    pub fn start(pipes: &Pipes, setup: Setup) -> io::Result<Self> {
        let reader =
            File::from(open(pipes.request(), OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::empty())?);
        let stop = Arc::new(AtomicBool::new(false));
        let answer = pipes.answer();
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || serve(BufReader::new(reader), &answer, &flag, &setup));
        Ok(Self { stop, request: pipes.request(), thread: Some(thread) })
    }

    /// Stops relaying once the request in hand, if any, is answered.
    pub fn stop(mut self) {
        self.end();
    }

    fn end(&mut self) {
        let Some(thread) = self.thread.take() else { return };
        self.stop.store(true, Ordering::SeqCst);
        // An empty line wakes the thread from its wait for the next request.
        if let Ok(fd) = open(&self.request, OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()) {
            let _ = File::from(fd).write_all(b"\n");
        }
        let _ = thread.join();
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.end();
    }
}

/// Answers every request line from `requests` until told to stop.
fn serve(mut requests: impl BufRead, answer: &Path, stop: &AtomicBool, setup: &Setup) {
    loop {
        let Ok(line) = read_line(&mut requests) else { return };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if matches!(&line, Ok(text) if text.is_empty()) {
            continue;
        }
        // A shim that went away before its answer began leaves nobody to answer, and nothing runs.
        let Ok(out) = open_answer(answer) else { continue };
        let _ = respond(line, setup, &mut io::BufWriter::new(out));
    }
}

/// Carries out the request in `line` when the build is expected to make it, and writes the
/// answer to `out`.
fn respond(line: Result<String, Refusal>, setup: &Setup, out: &mut impl Write) -> io::Result<()> {
    let checked = line
        .and_then(|text| Request::parse(&text))
        .and_then(|request| setup.expected.check(&request).map(|()| request));
    let request = match checked {
        Ok(request) => request,
        Err(refusal) => return refuse(refusal, setup, out),
    };
    let mut failed = None;
    let outcome = setup.session.run(
        &request,
        setup.size,
        &|| false,
        &mut |text| {
            if failed.is_none()
                && let Err(error) = write_response(out, &Response::Line(text))
            {
                failed = Some(error);
            }
        },
        &mut |_| {},
    );
    let ending = match outcome {
        Outcome::Finished(ProcessOutcome::Finished { code }) => Response::Done(code),
        Outcome::Finished(ProcessOutcome::Cancelled) | Outcome::Lost(_) => Response::Done(None),
        Outcome::Refused(refusal) => return refuse(refusal, setup, out),
    };
    write_response(out, &ending)
}

/// Says why `refusal` happened, then the refusal.
fn refuse(refusal: Refusal, setup: &Setup, out: &mut impl Write) -> io::Result<()> {
    if let Some((_, text)) = setup.refusals.iter().find(|(known, _)| *known == refusal) {
        write_response(out, &Response::Line(text.clone()))?;
    }
    write_response(out, &Response::Refused(refusal))
}

fn write_response(out: &mut impl Write, response: &Response) -> io::Result<()> {
    writeln!(out, "{response}")?;
    out.flush()
}

/// Opens the answer pipe for writing when a shim holds it open for reading; fails at once when
/// none does. Writes then wait for the shim to read, as a pipe's do.
fn open_answer(path: &Path) -> io::Result<File> {
    let fd = open(path, OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::empty())?;
    let flags = fcntl_getfl(&fd)?;
    fcntl_setfl(&fd, flags - OFlags::NONBLOCK)?;
    Ok(File::from(fd))
}

/// Reads the next request line without its newline: a refusal for one too long or not UTF-8.
fn read_line(input: &mut impl BufRead) -> io::Result<Result<String, Refusal>> {
    let mut bytes = Vec::new();
    let limit = u64::try_from(MAX_LINE).unwrap_or(u64::MAX) + 1;
    Read::take(&mut *input, limit).read_until(b'\n', &mut bytes)?;
    if bytes.pop_if(|byte| *byte == b'\n').is_none() {
        if bytes.len() <= MAX_LINE {
            // The pipe is held open by the relay itself, so this is only an end the relay made.
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        input.skip_until(b'\n')?;
        return Ok(Err(Refusal::Values));
    }
    Ok(String::from_utf8(bytes).map_err(|_| Refusal::Values))
}

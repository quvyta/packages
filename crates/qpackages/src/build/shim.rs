//! `qpac --elevate-shim <folder> pacman …`: what paru and yay run in place of sudo.
//!
//! The shim reads the pacman call it was given, writes it as one helper request to the build's
//! `request` pipe and prints the lines of the answer as pacman would have printed them. It exits
//! with pacman's own exit code, so paru and yay see what they would have seen from sudo. It
//! draws nothing and asks nothing.
//!
//! The answer pipe is opened before the request is written, so the qpac running the build
//! always finds someone to answer. A qpac that is gone is noticed rather than waited for: the
//! request pipe then has nobody reading it.
//!
//! Shims take turns: each holds a lock on the build's folder from its request to the end of its
//! answer, which the qpac running the build marks by closing the pipe. Two shims reading the one
//! answer pipe at once would take each other's lines, and one that opened the pipe while the last
//! answer's end was still open would take that end for its own.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::path::Path;

use qpackages_core::build::shim::{self, Call};
use qpackages_core::helper::Response;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{FlockOperation, Mode, OFlags, fcntl_getfl, fcntl_setfl, flock, open};

use super::pipes::{self, ANSWER, REQUEST};

/// The exit code of a call that did not go through, as pacman's own for an error.
const FAILED: i32 = 1;

/// How long the shim waits for the answer to begin before it looks whether qpac is still there.
const LOOK_AGAIN: Timespec = Timespec { tv_sec: 1, tv_nsec: 0 };

/// Runs the shim with the arguments after its flag and returns the exit code.
#[must_use]
pub fn run(args: &[String]) -> i32 {
    run_to(args, &mut io::stdout().lock())
}

/// [`run`], printing the answer's lines to `out`.
pub fn run_to(args: &[String], out: &mut impl Write) -> i32 {
    let Some((folder, call)) = args.split_first() else { return FAILED };
    let line = match shim::parse(call) {
        Some(Call::Validate) => return 0,
        Some(Call::Request(request)) => request.to_string(),
        None => return FAILED,
    };
    let Some(uid) = crate::current_uid() else { return FAILED };
    let folder = Path::new(folder);
    if pipes::check(folder, uid).is_err() {
        return FAILED;
    }
    exchange(folder, &line, out).unwrap_or(FAILED)
}

/// Writes `line` to the request pipe in `folder` and copies the answer's lines to `out`.
/// Returns the exit code the answer ends with.
///
/// # Errors
///
/// Returns the error of a pipe that could not be opened, read or written, including the request
/// pipe of a qpac that is no longer there.
pub fn exchange(folder: &Path, line: &str, out: &mut impl Write) -> io::Result<i32> {
    // Held until the answer has ended; dropping it lets the next shim go.
    let turn = open(folder, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::empty())?;
    flock(&turn, FlockOperation::LockExclusive)?;
    let answer = open_pipe(&folder.join(ANSWER), OFlags::RDONLY)?;
    let mut request = File::from(open_pipe(&folder.join(REQUEST), OFlags::WRONLY)?);
    request.write_all(format!("{line}\n").as_bytes())?;
    drop(request);
    wait_for_answer(&answer, &folder.join(REQUEST))?;
    blocking(&answer)?;
    let mut answer = BufReader::new(File::from(answer));
    let mut text = String::new();
    // Without an ending, qpac went away before saying how the call ended.
    let mut code = FAILED;
    while {
        text.clear();
        answer.read_line(&mut text)? > 0
    } {
        match Response::parse(text.trim_end_matches(['\n', '\r'])) {
            Some(Response::Line(line)) => {
                writeln!(out, "{line}")?;
                out.flush()?;
            }
            Some(Response::Done(Some(ended))) => code = ended,
            Some(Response::Done(None) | Response::Refused(_)) => code = FAILED,
            // A build's root steps are pacman's; snapd is never on the other end of this pipe.
            Some(Response::Ready(_) | Response::SnapChange(_)) | None => {}
        }
    }
    Ok(code)
}

/// Opens the pipe at `path` without waiting for the other end, then makes its reads and writes
/// wait again. Opening a pipe for writing that nobody reads fails at once.
fn open_pipe(path: &Path, access: OFlags) -> io::Result<OwnedFd> {
    let fd = open(path, access | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::empty())?;
    if access == OFlags::WRONLY {
        blocking(&fd)?;
    }
    Ok(fd)
}

/// Makes reads and writes on `fd` wait.
fn blocking(fd: &OwnedFd) -> io::Result<()> {
    let flags = fcntl_getfl(fd)?;
    fcntl_setfl(fd, flags - OFlags::NONBLOCK)?;
    Ok(())
}

/// Waits until the answer begins: a line, or the other end opened and closed again. Every second
/// without one it looks whether the request pipe still has a reader; when it has none, qpac is
/// gone and no answer will come.
fn wait_for_answer(answer: &OwnedFd, request: &Path) -> io::Result<()> {
    loop {
        let mut fds = [PollFd::new(answer, PollFlags::IN)];
        if poll(&mut fds, Some(&LOOK_AGAIN))? > 0 {
            return Ok(());
        }
        drop(open_pipe(request, OFlags::WRONLY)?);
    }
}

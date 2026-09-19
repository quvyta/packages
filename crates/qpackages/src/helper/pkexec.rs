//! Starting the helper through polkit: `pkexec <qpac> --privileged-helper`.
//!
//! pkexec asks for the password itself, in the desktop's own polkit window or, without one, on the
//! controlling terminal, so the screen is handed to the terminal while it asks. The helper it
//! starts keeps running once it said it is ready, and its pipes are the framework's: requests go
//! out through the [`LiveChild`], and its lines come back as the application's messages, which
//! the screen hands over to the session's [`Connection`] through the sender [`connect`] returns.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};

use qframe::runtime::LiveChild;
use qpackages_core::helper::start_args;

use super::session::{Connection, Host};
use crate::reload::Lookup;
use crate::settings::PrivilegeTool;

/// The polkit program that runs another program as root.
pub const PKEXEC: &str = "pkexec";

/// The program that asks for administrator permission, as the setting and this machine decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    /// pkexec, at this path; the bare name when the setting insists on it and it was not found,
    /// so starting it fails with the system's own words.
    Pkexec(PathBuf),
    /// sudo, which warms its ticket on the terminal first.
    Sudo,
}

impl Tool {
    /// The tool `setting` asks for on the machine `lookup` searches: `auto` takes pkexec when
    /// polkit's program is there and sudo otherwise.
    #[must_use]
    pub fn choose(setting: PrivilegeTool, lookup: &Lookup) -> Self {
        match (setting, lookup(PKEXEC)) {
            (PrivilegeTool::Sudo, _) | (PrivilegeTool::Auto, None) => Self::Sudo,
            (_, Some(path)) => Self::Pkexec(path),
            (PrivilegeTool::Pkexec, None) => Self::Pkexec(PathBuf::from(PKEXEC)),
        }
    }
}

/// The arguments after `pkexec` that start the helper at `exe`, with `locale` passed on as for
/// sudo: pkexec clears the environment, so the helper would not know the user's language.
#[must_use]
pub fn args(exe: &Path, locale: Option<&str>) -> Vec<String> {
    let mut args = start_args(exe, locale);
    // The core's arguments are sudo's and begin with its `-n`; pkexec takes the program first.
    if args.first().is_some_and(|first| first == "-n") {
        args.remove(0);
    }
    args
}

/// The session's connection to a helper pkexec started, and where its later lines go: every
/// line of [`ChildLine::Line`](qframe::runtime::ChildLine) is sent there, and dropping the
/// sender once the helper ended tells the connection its output is over.
#[must_use]
pub fn connect(child: LiveChild) -> (Connection, Sender<io::Result<String>>) {
    let (sender, receiver) = mpsc::channel();
    let input = Input { child: child.clone(), pending: Vec::new() };
    (Connection::from_lines(Box::new(input), receiver, Box::new(Live(child))), sender)
}

/// The helper's standard input: whole lines go to the child as they are finished.
struct Input {
    child: LiveChild,
    /// What was written since the last newline.
    pending: Vec<u8>,
}

impl Write for Input {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.child.write_line(&String::from_utf8_lossy(&line[..end]))?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Input {
    /// Closes the helper's input for every holder of the child, so letting the connection go
    /// ends the helper even while the runtime still holds a clone. It runs as root, so closing
    /// its input is the only way to ask it to end.
    fn drop(&mut self) {
        self.child.close_stdin();
    }
}

/// The helper pkexec started, as far as the connection needs to know.
struct Live(LiveChild);

impl Host for Live {
    fn is_alive(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }

    /// The helper's error stream is the terminal, so there is nothing to collect.
    fn last_words(&mut self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use qframe::runtime::ProcessOutcome;
    use qpackages_core::helper::{Refusal, Request};

    use super::*;
    use crate::helper::session::{Outcome, Session, StartFailure};

    /// A session that can only take over helpers started elsewhere.
    fn session() -> Session {
        Session::new(Arc::new(|| Err(io::ErrorKind::Unsupported.into())))
    }

    fn with_pkexec(program: &str) -> Option<PathBuf> {
        (program == PKEXEC).then(|| PathBuf::from("/usr/bin/pkexec"))
    }

    fn without_pkexec(_: &str) -> Option<PathBuf> {
        None
    }

    #[test]
    fn auto_takes_pkexec_where_polkit_is_and_sudo_elsewhere() {
        assert_eq!(Tool::choose(PrivilegeTool::Auto, &with_pkexec), Tool::Pkexec("/usr/bin/pkexec".into()));
        assert_eq!(Tool::choose(PrivilegeTool::Auto, &without_pkexec), Tool::Sudo);
    }

    #[test]
    fn an_explicit_choice_is_kept() {
        assert_eq!(Tool::choose(PrivilegeTool::Sudo, &with_pkexec), Tool::Sudo);
        assert_eq!(Tool::choose(PrivilegeTool::Pkexec, &with_pkexec), Tool::Pkexec("/usr/bin/pkexec".into()));
        assert_eq!(Tool::choose(PrivilegeTool::Pkexec, &without_pkexec), Tool::Pkexec(PKEXEC.into()));
    }

    #[test]
    fn pkexec_gets_the_program_first_and_the_language_after_it() {
        let exe = Path::new("/usr/bin/qpac");
        assert_eq!(args(exe, None), ["/usr/bin/qpac", "--privileged-helper"]);
        assert_eq!(args(exe, Some("tr_TR.UTF-8")), ["/usr/bin/qpac", "--privileged-helper", "--lang", "tr_TR.UTF-8"]);
        assert_eq!(
            args(exe, Some("$(reboot)")),
            ["/usr/bin/qpac", "--privileged-helper"],
            "a strange locale stays out"
        );
    }

    #[test]
    fn requests_reach_the_child_as_whole_lines_however_they_are_written() {
        let (child, program) = LiveChild::for_tests();
        let mut input = Input { child, pending: Vec::new() };
        input.write_all(b"si").expect("written");
        assert!(program.written().is_empty(), "half a line is held back");
        input.write_all(b"ze 80 24\ninst").expect("written");
        input.write_all(b"all cowsay\n").expect("written");
        assert_eq!(program.written(), ["size 80 24", "install cowsay"]);
    }

    #[test]
    fn a_ready_helper_serves_the_session_and_letting_it_go_closes_its_input() {
        let (child, program) = LiveChild::for_tests();
        let (connection, sender) = connect(child);
        let session = session();
        assert_eq!(session.adopt(connection, "ready 1"), Ok(()));
        assert!(session.is_alive());
        sender.send(Ok("line hello".to_owned())).expect("sent");
        sender.send(Ok("done 0".to_owned())).expect("sent");
        let request = Request::Install(vec!["cowsay".to_owned()]);
        let mut lines = Vec::new();
        let outcome = session.run(&request, (70, 9), &|| false, &mut |line| lines.push(line));
        assert_eq!(outcome, Outcome::Finished(ProcessOutcome::Finished { code: Some(0) }));
        assert_eq!(lines, ["hello"]);
        assert_eq!(program.written(), ["size 70 9", "install cowsay"]);
        assert!(program.stdin_open());
        session.end();
        assert!(!program.stdin_open(), "the helper's input is closed");
    }

    #[test]
    fn a_helper_that_is_not_ready_is_let_go() {
        let (child, program) = LiveChild::for_tests();
        let (connection, _sender) = connect(child);
        let session = session();
        assert_eq!(session.adopt(connection, "refused not-root"), Err(StartFailure::Refused(Refusal::NotRoot)));
        assert!(!session.is_alive());
        assert!(!program.stdin_open());
    }
}

//! How the application runs other programs: for real on this machine, or from a recording.
//!
//! Every command the application runs goes through a [`Runner`], so a test can hand it recorded
//! answers instead of letting it reach `sudo` or `pacman`. Two shapes of call exist: a short
//! query read to its end, and a long command streamed line by line while the screen stays alive.

use std::io;
use std::process::{Command, Stdio};

use qframe::runtime::{Line, Process, ProcessOutcome};

/// What a short query printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Its standard output, as text.
    pub stdout: String,
    /// Its standard error, as text.
    pub stderr: String,
    /// Its exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
}

impl Output {
    /// Whether the program ended with exit code 0.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs programs for the application.
///
/// `args` never carries the program name; `env` sets variables on top of the inherited
/// environment, so a call whose output is parsed pins the locale and a call the user watches
/// keeps theirs.
pub trait Runner: Send + Sync {
    /// Runs a short query to its end and returns what it printed and its exit code.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the program cannot be started.
    fn output(&self, program: &str, args: &[String], env: &[(&str, &str)]) -> io::Result<Output>;

    /// Runs a long command, handing every line it prints to `on_line` as it arrives. With `pty`
    /// the command runs on a pseudo-terminal of that many columns and rows, so it draws progress
    /// as it would on a real one; `cancel` is asked between lines and ends the command early.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the program cannot be started or the pseudo-terminal cannot
    /// be opened.
    fn stream(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
        pty: Option<(u16, u16)>,
        cancel: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(String),
    ) -> io::Result<ProcessOutcome>;
}

/// The runner of the installed application: it starts the real programs on this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Real;

impl Runner for Real {
    fn output(&self, program: &str, args: &[String], env: &[(&str, &str)]) -> io::Result<Output> {
        let mut command = Command::new(program);
        command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output()?;
        Ok(Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        })
    }

    fn stream(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
        pty: Option<(u16, u16)>,
        cancel: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(String),
    ) -> io::Result<ProcessOutcome> {
        let mut process = Process::new(program).args(args);
        for (key, value) in env {
            process = process.env(*key, *value);
        }
        if let Some((cols, rows)) = pty {
            process = process.pty(cols, rows);
        }
        // On a pseudo-terminal both streams share one line; on pipes an error line reads the
        // same to the user, so the difference is not carried further.
        process.run(cancel, &mut |line| match line {
            Line::Out(text) | Line::Err(text) => on_line(text),
        })
    }
}

/// A runner that answers from a table and records what it was asked, for tests that must never
/// reach the real package manager.
#[cfg(test)]
pub use recorded::{Call, Recorded};

#[cfg(test)]
mod recorded {
    use std::collections::HashMap;
    use std::io;
    use std::sync::{Mutex, PoisonError};

    use qframe::runtime::ProcessOutcome;

    use super::{Output, Runner};

    /// One program invocation, as the application asked for it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Call {
        /// The program.
        pub program: String,
        /// Its arguments, without the program name.
        pub args: Vec<String>,
        /// The variables set for it.
        pub env: Vec<(String, String)>,
        /// The pseudo-terminal size asked for, `None` for a query or a piped stream.
        pub pty: Option<(u16, u16)>,
    }

    /// The key of the answer table: the program and its exact arguments.
    type Key = (String, Vec<String>);

    /// Answers queries and streams from what the test recorded, and keeps every call.
    ///
    /// A call nobody recorded fails like a program that is not installed, so a test sees at
    /// once which command the application tried to run.
    #[derive(Debug, Default)]
    pub struct Recorded {
        outputs: Mutex<HashMap<Key, Output>>,
        streams: Mutex<HashMap<Key, (Vec<String>, ProcessOutcome)>>,
        calls: Mutex<Vec<Call>>,
    }

    impl Recorded {
        /// Answers `program args` with `stdout` and exit code `code` from now on.
        pub fn answer(&self, program: &str, args: &[impl AsRef<str>], stdout: &str, code: i32) {
            let output = Output { stdout: stdout.to_owned(), stderr: String::new(), code: Some(code) };
            self.outputs.lock().unwrap_or_else(PoisonError::into_inner).insert(key(program, args), output);
        }

        /// Answers `program args` with nothing on standard output, `stderr` and exit code `code`.
        pub fn fail(&self, program: &str, args: &[impl AsRef<str>], stderr: &str, code: i32) {
            let output = Output { stdout: String::new(), stderr: stderr.to_owned(), code: Some(code) };
            self.outputs.lock().unwrap_or_else(PoisonError::into_inner).insert(key(program, args), output);
        }

        /// Plays `lines` when `program args` is streamed, then ends with `outcome`.
        pub fn play(&self, program: &str, args: &[impl AsRef<str>], lines: &[&str], outcome: ProcessOutcome) {
            let lines = lines.iter().map(|line| (*line).to_owned()).collect();
            self.streams.lock().unwrap_or_else(PoisonError::into_inner).insert(key(program, args), (lines, outcome));
        }

        /// Every call so far, oldest first.
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }

        /// The calls so far as `program arg arg…` lines, for short assertions.
        pub fn command_lines(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .map(|call| std::iter::once(call.program).chain(call.args).collect::<Vec<_>>().join(" "))
                .collect()
        }

        fn record(&self, program: &str, args: &[String], env: &[(&str, &str)], pty: Option<(u16, u16)>) {
            let env = env.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect();
            let call = Call { program: program.to_owned(), args: args.to_vec(), env, pty };
            self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(call);
        }
    }

    fn key(program: &str, args: &[impl AsRef<str>]) -> Key {
        (program.to_owned(), args.iter().map(|arg| arg.as_ref().to_owned()).collect())
    }

    fn not_recorded(program: &str, args: &[String]) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, format!("no recording for `{program} {}`", args.join(" ")))
    }

    impl Runner for Recorded {
        fn output(&self, program: &str, args: &[String], env: &[(&str, &str)]) -> io::Result<Output> {
            self.record(program, args, env, None);
            let outputs = self.outputs.lock().unwrap_or_else(PoisonError::into_inner);
            outputs.get(&key(program, args)).cloned().ok_or_else(|| not_recorded(program, args))
        }

        fn stream(
            &self,
            program: &str,
            args: &[String],
            env: &[(&str, &str)],
            pty: Option<(u16, u16)>,
            cancel: &dyn Fn() -> bool,
            on_line: &mut dyn FnMut(String),
        ) -> io::Result<ProcessOutcome> {
            self.record(program, args, env, pty);
            let (lines, outcome) = {
                let streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
                streams.get(&key(program, args)).cloned().ok_or_else(|| not_recorded(program, args))?
            };
            for line in lines {
                if cancel() {
                    return Ok(ProcessOutcome::Cancelled);
                }
                on_line(line);
            }
            Ok(outcome)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_runner_reads_a_short_query_to_its_end() {
        let args = ["-c", "echo out; echo err >&2; exit 3"].map(str::to_owned);
        let output = Real.output("sh", &args, &[("LC_ALL", "C")]).expect("the shell starts");
        assert_eq!(output, Output { stdout: "out\n".to_owned(), stderr: "err\n".to_owned(), code: Some(3) });
        assert!(!output.succeeded());
    }

    #[test]
    fn the_real_runner_streams_lines_with_the_environment_it_was_given() {
        // Both lines go to standard output: two pipes are read on two threads, in no fixed order.
        let args = ["-c", "echo $QPACKAGES_RUNNER_TEST; echo two"].map(str::to_owned);
        let mut lines = Vec::new();
        let outcome = Real
            .stream("sh", &args, &[("QPACKAGES_RUNNER_TEST", "one")], None, &|| false, &mut |line| lines.push(line))
            .expect("the shell starts");
        assert_eq!(outcome, ProcessOutcome::Finished { code: Some(0) });
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_panic() {
        assert!(Real.output("qpackages-no-such-program", &[], &[]).is_err());
        assert!(Real.stream("qpackages-no-such-program", &[], &[], None, &|| false, &mut |_| {}).is_err());
    }

    #[test]
    fn the_recorded_runner_answers_from_its_table_and_keeps_every_call() {
        let recorded = Recorded::default();
        recorded.answer("pacman", &["-Q"], "bash 5.3\n", 0);
        recorded.play("sudo", &["pacman", "-S"], &["one", "two"], ProcessOutcome::Finished { code: Some(0) });
        let query = recorded.output("pacman", &["-Q".to_owned()], &[("LC_ALL", "C")]).expect("recorded");
        assert_eq!(query.stdout, "bash 5.3\n");
        let mut lines = Vec::new();
        let args = ["pacman", "-S"].map(str::to_owned);
        let outcome = recorded.stream("sudo", &args, &[], Some((80, 24)), &|| false, &mut |l| lines.push(l));
        assert_eq!(outcome.expect("recorded"), ProcessOutcome::Finished { code: Some(0) });
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(recorded.command_lines(), ["pacman -Q", "sudo pacman -S"]);
        assert_eq!(recorded.calls()[0].env, [("LC_ALL".to_owned(), "C".to_owned())]);
        assert_eq!(recorded.calls()[1].pty, Some((80, 24)));
        let error = recorded.output("pacman", &["-Sy".to_owned()], &[]).expect_err("nothing was recorded for it");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_recorded_stream_stops_when_cancelled() {
        let recorded = Recorded::default();
        recorded.play("sudo", &["pacman"], &["one", "two"], ProcessOutcome::Finished { code: Some(0) });
        let seen = std::cell::Cell::new(0);
        let outcome = recorded
            .stream("sudo", &["pacman".to_owned()], &[], None, &|| seen.get() > 0, &mut |_| seen.set(seen.get() + 1))
            .expect("recorded");
        assert_eq!(outcome, ProcessOutcome::Cancelled);
        assert_eq!(seen.get(), 1);
    }
}

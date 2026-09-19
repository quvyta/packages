//! The helper's root side: `qpac --privileged-helper [--lang <locale>]`.
//!
//! It draws nothing. It reads requests line by line, checks each one completely before running
//! anything, runs pacman by its absolute path with a fixed argument list and its own
//! environment, never through a shell, and streams pacman's lines back. When its input ends it
//! has already finished the job in hand, because a job runs to its end before the next line is
//! read: pacman stopped halfway can leave its database broken.

use std::io::{self, BufRead, Read, Write};

use qframe::runtime::ProcessOutcome;
use qpackages_core::helper::{PACMAN_PATH, Refusal, Request, Response, VERSION, pacman_env, parse_options};

use crate::runner::{Real, Runner};

/// The user id of root.
const ROOT: u32 = 0;

/// The longest request line read, in bytes: far more than any list of package names needs, and
/// a bound on what a runaway writer can make the helper hold.
const MAX_LINE: usize = 1 << 20;

/// The pseudo-terminal pacman gets before a `size` request says otherwise.
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// Runs the helper on this process's standard input and output. `args` are the arguments after
/// the helper flag.
///
/// # Errors
///
/// Returns the error of a read from standard input or a write to standard output that failed.
pub fn run(args: &[String]) -> io::Result<()> {
    answer(args, crate::current_uid(), io::stdin().lock(), io::stdout().lock(), &Real)
}

/// Runs the helper as user `uid` with arguments `args`, reading `input` and writing `output`.
///
/// Anything that is not root is refused before the first request is read, and so is a start
/// with arguments other than a valid `--lang`.
///
/// # Errors
///
/// Returns the error of a read from `input` or of a response that could not be written.
pub fn answer(
    args: &[String],
    uid: Option<u32>,
    input: impl BufRead,
    mut output: impl Write,
    runner: &dyn Runner,
) -> io::Result<()> {
    if uid != Some(ROOT) {
        return respond(&mut output, &Response::Refused(Refusal::NotRoot));
    }
    match parse_options(args) {
        Ok(locale) => serve(input, output, runner, &locale),
        Err(refusal) => respond(&mut output, &Response::Refused(refusal)),
    }
}

/// Announces the helper, then answers every request from `input` on `output` until `input`
/// ends, running pacman through `runner` with its shown output in `locale`.
///
/// A last line without its newline is not carried out: the writer may have gone away halfway
/// through it.
///
/// # Errors
///
/// Returns the error of a read from `input` or of a response that could not be written.
fn serve(mut input: impl BufRead, mut output: impl Write, runner: &dyn Runner, locale: &str) -> io::Result<()> {
    respond(&mut output, &Response::Ready(VERSION))?;
    let env = pacman_env(locale);
    let mut size = DEFAULT_SIZE;
    while let Some(line) = read_line(&mut input)? {
        let request = line.and_then(|text| Request::parse(&text));
        let response = match request {
            Ok(Request::Size { cols, rows }) => {
                size = (cols, rows);
                continue;
            }
            Ok(request) => match request.pacman_args() {
                Some(args) => carry_out(&args, &env, size, runner, &mut output),
                None => continue,
            },
            Err(refusal) => Response::Refused(refusal),
        };
        respond(&mut output, &response)?;
    }
    Ok(())
}

/// Runs pacman with `args` on a `size` pseudo-terminal, streaming its lines to `output`, and
/// says how it ended.
fn carry_out(
    args: &[String],
    env: &[(&str, &str)],
    size: (u16, u16),
    runner: &dyn Runner,
    output: &mut impl Write,
) -> Response {
    // A line that cannot be written is dropped: pacman runs to its end whether or not anyone is
    // still listening.
    let outcome = runner.stream(PACMAN_PATH, args, env, Some(size), &|| false, &mut |text| {
        let _ = respond(output, &Response::Line(text));
    });
    match outcome {
        Ok(ProcessOutcome::Finished { code }) => Response::Done(code),
        Ok(ProcessOutcome::Cancelled) => Response::Done(None),
        Err(_) => Response::Refused(Refusal::Start),
    }
}

/// Reads the next request line without its newline. `None` when the input ends, including after
/// a last line without a newline; a refusal for a line that is too long or not UTF-8.
fn read_line(input: &mut impl BufRead) -> io::Result<Option<Result<String, Refusal>>> {
    let mut bytes = Vec::new();
    let limit = u64::try_from(MAX_LINE).unwrap_or(u64::MAX) + 1;
    Read::take(&mut *input, limit).read_until(b'\n', &mut bytes)?;
    if bytes.pop_if(|byte| *byte == b'\n').is_none() {
        if bytes.len() <= MAX_LINE {
            return Ok(None);
        }
        input.skip_until(b'\n')?;
        return Ok(Some(Err(Refusal::Values)));
    }
    Ok(Some(String::from_utf8(bytes).map_err(|_| Refusal::Values)))
}

/// Writes one response line and sends it at once, so the other side sees it while pacman works.
fn respond(output: &mut impl Write, response: &Response) -> io::Result<()> {
    writeln!(output, "{response}")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Recorded;

    /// Serves `input` with `recorded` and returns what the helper wrote, line by line.
    fn served(recorded: &Recorded, input: &[u8]) -> Vec<String> {
        let mut output = Vec::new();
        serve(input, &mut output, recorded, "tr_TR.UTF-8").expect("the helper serves to the end");
        String::from_utf8(output).expect("responses are text").lines().map(str::to_owned).collect()
    }

    fn install_args() -> Vec<String> {
        ["-S", "--needed", "--noconfirm", "--", "cowsay"].map(str::to_owned).to_vec()
    }

    #[test]
    fn only_root_with_valid_options_is_served() {
        let recorded = Recorded::default();
        let answered = |args: &[&str], uid| {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            let mut output = Vec::new();
            answer(&args, uid, &b"install cowsay\n"[..], &mut output, &recorded).expect("answered");
            String::from_utf8(output).expect("text")
        };
        assert_eq!(answered(&[], Some(1000)), "refused not-root\n");
        assert_eq!(answered(&[], None), "refused not-root\n");
        assert_eq!(answered(&["--lang", "C;id"], Some(0)), "refused values\n");
        assert_eq!(answered(&["--lang", "C"], Some(0)), "ready 1\nrefused start\n");
        assert_eq!(recorded.calls().len(), 1, "only root ran anything");
    }

    #[test]
    fn a_request_runs_pacman_by_its_path_and_streams_its_lines() {
        let recorded = Recorded::default();
        let lines = [":: Processing package changes...", "(1/1) installing cowsay"];
        recorded.play(PACMAN_PATH, &install_args(), &lines, ProcessOutcome::Finished { code: Some(0) });
        let out = served(&recorded, b"size 100 30\ninstall cowsay\n");
        assert_eq!(out, ["ready 1", "line :: Processing package changes...", "line (1/1) installing cowsay", "done 0"]);
        let call = &recorded.calls()[0];
        assert_eq!(call.program, "/usr/bin/pacman");
        assert_eq!(call.args, install_args());
        assert_eq!(call.pty, Some((100, 30)));
        let env: Vec<(&str, &str)> = call.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(env, [("PATH", "/usr/bin:/usr/sbin"), ("LANG", "tr_TR.UTF-8"), ("LC_ALL", "tr_TR.UTF-8")]);
    }

    #[test]
    fn requests_follow_one_another_and_keep_the_last_size() {
        let recorded = Recorded::default();
        let remove = ["-Rns", "--noconfirm", "--", "yay"];
        recorded.play(PACMAN_PATH, &install_args(), &[], ProcessOutcome::Finished { code: Some(0) });
        recorded.play(PACMAN_PATH, &remove, &["removing"], ProcessOutcome::Finished { code: Some(1) });
        let out = served(&recorded, b"install cowsay\nsize 60 9\nremove yay\n");
        assert_eq!(out, ["ready 1", "done 0", "line removing", "done 1"]);
        let sizes: Vec<_> = recorded.calls().iter().map(|call| call.pty).collect();
        assert_eq!(sizes, [Some((80, 24)), Some((60, 9))], "the default size, then the one asked for");
    }

    #[test]
    fn refused_requests_run_nothing_and_the_helper_carries_on() {
        let recorded = Recorded::default();
        recorded.play(PACMAN_PATH, &install_args(), &[], ProcessOutcome::Finished { code: Some(0) });
        let input = b"upgrade\ninstall\ninstall --overwrite=*\nsize 0 0\nsh -c id\ninstall cowsay\n";
        let out = served(&recorded, input);
        assert_eq!(
            out,
            [
                "ready 1",
                "refused unknown",
                "refused values",
                "refused name",
                "refused values",
                "refused unknown",
                "done 0"
            ]
        );
        assert_eq!(recorded.calls().len(), 1, "only the valid request ran");
    }

    #[test]
    fn a_last_line_without_its_newline_is_not_carried_out() {
        let recorded = Recorded::default();
        recorded.play(PACMAN_PATH, &install_args(), &[], ProcessOutcome::Finished { code: Some(0) });
        assert_eq!(served(&recorded, b"install cowsay"), ["ready 1"]);
        assert_eq!(served(&recorded, b""), ["ready 1"]);
        assert!(recorded.calls().is_empty());
    }

    #[test]
    fn a_line_too_long_or_not_text_is_refused_and_the_next_one_is_read() {
        let recorded = Recorded::default();
        recorded.play(PACMAN_PATH, &install_args(), &[], ProcessOutcome::Finished { code: Some(0) });
        let mut input = b"install ".to_vec();
        input.extend(std::iter::repeat_n(b'a', MAX_LINE + 10));
        input.extend(b"\ninstall \xff\ninstall cowsay\n");
        assert_eq!(served(&recorded, &input), ["ready 1", "refused values", "refused values", "done 0"]);
        assert_eq!(recorded.calls().len(), 1);
    }

    #[test]
    fn pacman_that_cannot_start_or_ends_by_a_signal_is_reported() {
        let recorded = Recorded::default();
        assert_eq!(served(&recorded, b"install cowsay\n"), ["ready 1", "refused start"]);
        recorded.play(PACMAN_PATH, &install_args(), &[], ProcessOutcome::Finished { code: None });
        assert_eq!(served(&recorded, b"install cowsay\n"), ["ready 1", "done signal"]);
    }

    #[test]
    fn a_gone_listener_does_not_stop_the_job() {
        /// Output that accepts only the ready line, as a pipe whose reader went away.
        struct Closing(usize);
        impl Write for Closing {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                if buf.contains(&b'\n') {
                    self.0 -= 1;
                }
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let recorded = Recorded::default();
        recorded.play(PACMAN_PATH, &install_args(), &["one", "two"], ProcessOutcome::Finished { code: Some(0) });
        let result = serve(&b"install cowsay\n"[..], Closing(1), &recorded, "C");
        assert_eq!(result.map_err(|error| error.kind()), Err(io::ErrorKind::BrokenPipe), "the end cannot be said");
        assert_eq!(recorded.calls().len(), 1, "pacman ran to its end all the same");
    }
}

//! The helper's root side: `qpac --privileged-helper [--lang <locale>]`.
//!
//! It draws nothing. It reads requests line by line, checks each one completely before running
//! anything, runs pacman and the few programs around it by their absolute paths with fixed
//! argument lists and its own environment, never through a shell, and streams their lines back. When its input ends it
//! has already finished the job in hand, because a job runs to its end before the next line is
//! read: pacman stopped halfway can leave its database broken.

use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;

use qframe::runtime::ProcessOutcome;
use qpackages_core::helper::{PACMAN_PATH, Refusal, Request, Response, VERSION, pacman_env, parse_options};
use qpackages_core::pacman::{command, read_orphans};
use qpackages_core::reflector::{BACKUP, CONFIG, MIRRORLIST, Mirrors, REFLECTOR_PATH, has_server};

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
    answer(args, crate::current_uid(), Path::new("/"), io::stdin().lock(), io::stdout().lock(), &Real)
}

/// Runs the helper as user `uid` with arguments `args`, reading `input` and writing `output`.
/// The files it writes itself, such as the mirror list, are under `root`: `/` on a real
/// machine.
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
    root: &Path,
    input: impl BufRead,
    mut output: impl Write,
    runner: &dyn Runner,
) -> io::Result<()> {
    if uid != Some(ROOT) {
        return respond(&mut output, &Response::Refused(Refusal::NotRoot));
    }
    match parse_options(args) {
        Ok(locale) => serve(root, input, output, runner, &locale),
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
fn serve(
    root: &Path,
    mut input: impl BufRead,
    mut output: impl Write,
    runner: &dyn Runner,
    locale: &str,
) -> io::Result<()> {
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
            Ok(Request::RemoveOrphans(expected)) => remove_orphans(&expected, &env, size, runner, &mut output),
            Ok(Request::Mirrors(mirrors)) => choose_mirrors(root, &mirrors, &env, size, runner, &mut output),
            Ok(request) => match request.command() {
                Some((program, args)) => carry_out(program, &args, &env, size, runner, &mut output),
                None => continue,
            },
            Err(refusal) => Response::Refused(refusal),
        };
        respond(&mut output, &response)?;
    }
    Ok(())
}

/// Lists the orphans with pacman itself and removes them, but only when they are exactly the
/// `expected` ones qpac showed: a transaction in between may have made a package the user saw
/// as needed an orphan, and nothing the user did not see is removed.
fn remove_orphans(
    expected: &[String],
    env: &[(&str, &str)],
    size: (u16, u16),
    runner: &dyn Runner,
    output: &mut impl Write,
) -> Response {
    // The list is parsed, so this one query runs in the C locale whatever the shown output uses.
    let Ok(listed) = runner.output(PACMAN_PATH, &command::orphans(), &pacman_env("C")) else {
        return Response::Refused(Refusal::Start);
    };
    let Ok(found) = read_orphans(listed.code, &listed.stdout, &listed.stderr) else {
        return Response::Refused(Refusal::Start);
    };
    let mut expected = expected.to_vec();
    expected.sort();
    expected.dedup();
    if found != expected {
        return Response::Refused(Refusal::Changed);
    }
    carry_out(PACMAN_PATH, &command::remove(&found), env, size, runner, output)
}

/// Runs reflector into a new file beside pacman's mirror list under `root`, and puts that file in
/// place only when reflector ended well and the list names a server; the list it replaces is
/// kept as [`BACKUP`]. The same choices then go to reflector's configuration, for its timer.
fn choose_mirrors(
    root: &Path,
    mirrors: &Mirrors,
    env: &[(&str, &str)],
    size: (u16, u16),
    runner: &dyn Runner,
    output: &mut impl Write,
) -> Response {
    let list = root.join(MIRRORLIST);
    let Some(folder) = list.parent() else { return Response::Refused(Refusal::File) };
    let fresh = folder.join(".mirrorlist.qpac-new");
    // A file left by an interrupted run must never pass for reflector's new output.
    if fs::remove_file(&fresh).is_err_and(|error| error.kind() != io::ErrorKind::NotFound) {
        return Response::Refused(Refusal::File);
    }
    let ran = carry_out(REFLECTOR_PATH, &mirrors.args(&fresh), env, size, runner, output);
    if ran != Response::Done(Some(0)) {
        let _ = fs::remove_file(&fresh);
        return ran;
    }
    if !fs::read_to_string(&fresh).is_ok_and(|text| has_server(&text)) {
        let _ = fs::remove_file(&fresh);
        return Response::Refused(Refusal::NoMirrors);
    }
    // The timer's configuration goes first: when it cannot be written, nothing has changed yet.
    let replaced = write_atomically(&root.join(CONFIG), &mirrors.config())
        .and_then(|()| keep_old_list(&list, &folder.join(BACKUP)))
        .and_then(|()| fs::set_permissions(&fresh, std::os::unix::fs::PermissionsExt::from_mode(0o644)))
        .and_then(|()| fs::rename(&fresh, &list));
    if replaced.is_err() {
        let _ = fs::remove_file(&fresh);
        return Response::Refused(Refusal::File);
    }
    ran
}

/// Copies the mirror list at `list` to `backup`; a machine without a list has nothing to keep.
fn keep_old_list(list: &Path, backup: &Path) -> io::Result<()> {
    match fs::copy(list, backup) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Writes `text` to `path` through a file beside it, so a reader sees the old text or the new,
/// never half of one.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let folder = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(folder)?;
    let fresh = folder.join(".qpac-new");
    fs::write(&fresh, text)?;
    fs::set_permissions(&fresh, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
    fs::rename(&fresh, path)
}

/// Runs `program` with `args` on a `size` pseudo-terminal, streaming its lines to `output`, and
/// says how it ended.
fn carry_out(
    program: &str,
    args: &[String],
    env: &[(&str, &str)],
    size: (u16, u16),
    runner: &dyn Runner,
    output: &mut impl Write,
) -> Response {
    // A line that cannot be written is dropped: pacman runs to its end whether or not anyone is
    // still listening.
    let outcome = runner.stream(program, args, env, Some(size), &|| false, &mut |text| {
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
    use qpackages_core::backup::Snapshot;
    use qpackages_core::reflector::Mirrors;

    use super::*;
    use crate::runner::Recorded;

    /// Serves `input` with `recorded` and returns what the helper wrote, line by line.
    fn served(recorded: &Recorded, input: &[u8]) -> Vec<String> {
        let mut output = Vec::new();
        serve(&nowhere(), input, &mut output, recorded, "tr_TR.UTF-8").expect("the helper serves to the end");
        String::from_utf8(output).expect("responses are text").lines().map(str::to_owned).collect()
    }

    /// A root under which nothing exists, for tests whose requests write no files.
    fn nowhere() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("qpackages-helper-nowhere-{}", std::process::id()))
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
            answer(&args, uid, &nowhere(), &b"install cowsay\n"[..], &mut output, &recorded).expect("answered");
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
        let input = b"upgrade now\ninstall\ninstall --overwrite=*\nsize 0 0\nsh -c id\ninstall cowsay\n";
        let out = served(&recorded, input);
        assert_eq!(
            out,
            [
                "ready 1",
                "refused values",
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
    fn upgrades_marks_and_the_timer_run_their_fixed_commands() {
        let recorded = Recorded::default();
        let done = ProcessOutcome::Finished { code: Some(0) };
        recorded.play(PACMAN_PATH, &["-Syu", "--noconfirm"], &["upgrading"], done.clone());
        recorded.play(PACMAN_PATH, &["-Syu", "--needed", "--noconfirm", "--", "vlc"], &[], done.clone());
        recorded.play(PACMAN_PATH, &["-D", "--asdeps", "--", "vlc"], &[], done.clone());
        recorded.play(PACMAN_PATH, &["-D", "--asexplicit", "--", "vlc"], &[], done.clone());
        recorded.play("/usr/bin/systemctl", &["enable", "--now", "--", "reflector.timer"], &[], done.clone());
        recorded.play("/usr/bin/systemctl", &["disable", "--now", "--", "reflector.timer"], &[], done);
        let input = b"upgrade\nupgrade-install vlc\nmark-deps vlc\nmark-explicit vlc\ntimer on reflector.timer\n\
            timer off reflector.timer\n";
        let out = served(&recorded, input);
        assert_eq!(out, ["ready 1", "line upgrading", "done 0", "done 0", "done 0", "done 0", "done 0", "done 0"]);
        assert_eq!(
            recorded.command_lines(),
            [
                "/usr/bin/pacman -Syu --noconfirm",
                "/usr/bin/pacman -Syu --needed --noconfirm -- vlc",
                "/usr/bin/pacman -D --asdeps -- vlc",
                "/usr/bin/pacman -D --asexplicit -- vlc",
                "/usr/bin/systemctl enable --now -- reflector.timer",
                "/usr/bin/systemctl disable --now -- reflector.timer",
            ]
        );
    }

    #[test]
    fn orphans_are_removed_only_when_they_are_the_ones_shown() {
        let recorded = Recorded::default();
        recorded.answer(PACMAN_PATH, &["-Qtdq"], "python-wheel\nlibfoo\n", 0);
        let remove = ["-Rns", "--noconfirm", "--", "libfoo", "python-wheel"];
        recorded.play(PACMAN_PATH, &remove, &["removing"], ProcessOutcome::Finished { code: Some(0) });
        let out = served(&recorded, b"remove-orphans python-wheel libfoo\n");
        assert_eq!(out, ["ready 1", "line removing", "done 0"], "the order qpac sent them in does not matter");
        let query = &recorded.calls()[0];
        assert_eq!(query.args, ["-Qtdq"]);
        let env: Vec<(&str, &str)> = query.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(env, [("PATH", "/usr/bin:/usr/sbin"), ("LANG", "C"), ("LC_ALL", "C")], "the list is parsed");
        assert_eq!(recorded.calls()[1].args, remove, "the list the helper found, after a double dash");
    }

    #[test]
    fn a_changed_orphan_list_is_refused_and_nothing_is_removed() {
        let recorded = Recorded::default();
        recorded.answer(PACMAN_PATH, &["-Qtdq"], "libfoo\nlibbar\n", 0);
        for line in [&b"remove-orphans libfoo\n"[..], b"remove-orphans libfoo libbar libbaz\n"] {
            assert_eq!(served(&recorded, line), ["ready 1", "refused changed"]);
        }
        recorded.answer(PACMAN_PATH, &["-Qtdq"], "", 1);
        assert_eq!(served(&recorded, b"remove-orphans libfoo\n"), ["ready 1", "refused changed"], "none are left");
        assert!(recorded.calls().iter().all(|call| call.args == ["-Qtdq"]), "only the query ran");
    }

    #[test]
    fn an_orphan_query_that_fails_is_refused() {
        let recorded = Recorded::default();
        assert_eq!(served(&recorded, b"remove-orphans libfoo\n"), ["ready 1", "refused start"], "pacman missing");
        recorded.fail(PACMAN_PATH, &["-Qtdq"], "error: failed to init transaction", 1);
        assert_eq!(served(&recorded, b"remove-orphans libfoo\n"), ["ready 1", "refused start"]);
        assert_eq!(recorded.calls().len(), 2, "only the queries ran");
    }

    #[test]
    fn snapshots_run_the_tools_fixed_commands() {
        let recorded = Recorded::default();
        let done = ProcessOutcome::Finished { code: Some(0) };
        let (snapper, pre) = Snapshot::SnapperPre.command();
        recorded.play(snapper, &pre, &["42"], done.clone());
        let (snapper, post) = Snapshot::SnapperPost(42).command();
        recorded.play(snapper, &post, &[], done.clone());
        let (timeshift, create) = Snapshot::Timeshift.command();
        recorded.play(timeshift, &create, &["Creating new snapshot..."], done);
        let input = b"snapshot pre snapper\nsnapshot post snapper 42\nsnapshot pre timeshift\nsnapshot pre x\n";
        let out = served(&recorded, input);
        assert_eq!(
            out,
            ["ready 1", "line 42", "done 0", "done 0", "line Creating new snapshot...", "done 0", "refused values"]
        );
        let programs: Vec<String> = recorded.calls().into_iter().map(|call| call.program).collect();
        assert_eq!(programs, ["/usr/bin/snapper", "/usr/bin/snapper", "/usr/bin/timeshift"]);
    }

    /// Plays reflector from `recorded` and, like reflector, writes `list` to the file after
    /// `--save` when there is one.
    struct Reflector {
        recorded: Recorded,
        list: Option<&'static str>,
    }

    impl Runner for Reflector {
        fn output(&self, program: &str, args: &[String], env: &[(&str, &str)]) -> io::Result<crate::runner::Output> {
            self.recorded.output(program, args, env)
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
            let outcome = self.recorded.stream(program, args, env, pty, cancel, on_line)?;
            if let (Some(list), [save, path, ..]) = (self.list, args) {
                assert_eq!(save, "--save");
                fs::write(path, list)?;
            }
            Ok(outcome)
        }
    }

    /// A machine root with pacman's mirror list holding `old`, when given.
    fn machine(name: &str, old: Option<&str>) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("qpackages-helper-mirrors-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("etc/pacman.d")).expect("the mirror list's folder");
        if let Some(old) = old {
            fs::write(root.join(MIRRORLIST), old).expect("the old list");
        }
        root
    }

    fn reflector(root: &Path, list: Option<&'static str>, lines: &[&str], code: i32) -> Reflector {
        let recorded = Recorded::default();
        let args = Mirrors::default().args(&root.join("etc/pacman.d/.mirrorlist.qpac-new"));
        recorded.play(REFLECTOR_PATH, &args, lines, ProcessOutcome::Finished { code: Some(code) });
        Reflector { recorded, list }
    }

    fn serve_at(root: &Path, runner: &Reflector, input: &[u8]) -> Vec<String> {
        let mut output = Vec::new();
        serve(root, input, &mut output, runner, "C").expect("the helper serves to the end");
        String::from_utf8(output).expect("responses are text").lines().map(str::to_owned).collect()
    }

    const NEW: &str = "## Turkey\nServer = https://mirror.example.org/archlinux/$repo/os/$arch\n";

    #[test]
    fn a_new_mirror_list_replaces_the_old_one_which_is_kept() {
        let root = machine("replace", Some("Server = https://old.example.org/$repo/os/$arch\n"));
        let runner = reflector(&root, Some(NEW), &["rating mirrors"], 0);
        let out = serve_at(&root, &runner, b"mirrors https 12 10 rate\n");
        assert_eq!(out, ["ready 1", "line rating mirrors", "done 0"]);
        assert_eq!(fs::read_to_string(root.join(MIRRORLIST)).expect("the new list"), NEW);
        let kept = fs::read_to_string(root.join("etc/pacman.d").join(BACKUP)).expect("the kept list");
        assert!(kept.contains("old.example.org"));
        assert!(!root.join("etc/pacman.d/.mirrorlist.qpac-new").exists());
        let config = fs::read_to_string(root.join(CONFIG)).expect("reflector's configuration");
        assert!(config.contains("--save /etc/pacman.d/mirrorlist\n--protocol https\n"), "{config}");
        let call = &runner.recorded.calls()[0];
        assert_eq!(call.program, "/usr/bin/reflector");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_first_mirror_list_needs_nothing_to_keep() {
        let root = machine("first", None);
        let runner = reflector(&root, Some(NEW), &[], 0);
        assert_eq!(serve_at(&root, &runner, b"mirrors https 12 10 rate\n"), ["ready 1", "done 0"]);
        assert_eq!(fs::read_to_string(root.join(MIRRORLIST)).expect("the new list"), NEW);
        assert!(!root.join("etc/pacman.d").join(BACKUP).exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_list_without_servers_leaves_the_old_one_alone() {
        let root = machine("empty", Some("old\n"));
        let runner = reflector(&root, Some("## no mirrors matched\n"), &[], 0);
        assert_eq!(serve_at(&root, &runner, b"mirrors https 12 10 rate\n"), ["ready 1", "refused no-mirrors"]);
        assert_eq!(fs::read_to_string(root.join(MIRRORLIST)).expect("the old list"), "old\n");
        assert!(!root.join("etc/pacman.d").join(BACKUP).exists());
        assert!(!root.join("etc/pacman.d/.mirrorlist.qpac-new").exists());
        assert!(!root.join(CONFIG).exists(), "the timer keeps its old choices too");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_reflector_leaves_the_old_list_alone() {
        let root = machine("failed", Some("old\n"));
        let runner = reflector(&root, Some(NEW), &["error: failed to retrieve mirrorstatus data"], 1);
        let out = serve_at(&root, &runner, b"mirrors https 12 10 rate\n");
        assert_eq!(out, ["ready 1", "line error: failed to retrieve mirrorstatus data", "done 1"]);
        assert_eq!(fs::read_to_string(root.join(MIRRORLIST)).expect("the old list"), "old\n");
        assert!(!root.join("etc/pacman.d/.mirrorlist.qpac-new").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_file_left_by_an_earlier_run_never_passes_for_reflectors_output() {
        let root = machine("stale", Some("old\n"));
        fs::write(root.join("etc/pacman.d/.mirrorlist.qpac-new"), NEW).expect("a stale file");
        let runner = reflector(&root, None, &[], 0);
        assert_eq!(serve_at(&root, &runner, b"mirrors https 12 10 rate\n"), ["ready 1", "refused no-mirrors"]);
        assert_eq!(fs::read_to_string(root.join(MIRRORLIST)).expect("the old list"), "old\n");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_configuration_that_cannot_be_written_changes_nothing() {
        let root = machine("config", Some("old\n"));
        // A file where reflector's folder should be: nothing can be written under it.
        fs::create_dir_all(root.join("etc")).expect("etc");
        fs::write(root.join("etc/xdg"), "").expect("a file in the way");
        let runner = reflector(&root, Some(NEW), &[], 0);
        assert_eq!(serve_at(&root, &runner, b"mirrors https 12 10 rate\n"), ["ready 1", "refused file"]);
        assert_eq!(fs::read_to_string(root.join(MIRRORLIST)).expect("the old list"), "old\n");
        assert!(!root.join("etc/pacman.d/.mirrorlist.qpac-new").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_a_refusal() {
        let root = std::env::temp_dir().join(format!("qpackages-helper-mirrors-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let runner = reflector(&root, Some(NEW), &[], 0);
        let out = serve_at(&root, &runner, b"mirrors https 12 10 rate\n");
        assert_eq!(out, ["ready 1", "refused start"], "reflector's file cannot be written, so it did not end well");
        assert!(!root.exists());
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
        let result = serve(&nowhere(), &b"install cowsay\n"[..], Closing(1), &recorded, "C");
        assert_eq!(result.map_err(|error| error.kind()), Err(io::ErrorKind::BrokenPipe), "the end cannot be said");
        assert_eq!(recorded.calls().len(), 1, "pacman ran to its end all the same");
    }
}

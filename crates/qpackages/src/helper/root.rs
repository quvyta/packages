//! The helper's root side: `qpac --privileged-helper [--lang <locale>]`.
//!
//! It draws nothing. It reads requests line by line, checks each one completely before running
//! anything, runs pacman and the few programs around it by their absolute paths with fixed
//! argument lists and its own environment, never through a shell, and streams their lines back. When its input ends it
//! has already finished the job in hand, because a job runs to its end before the next line is
//! read: pacman stopped halfway can leave its database broken.

use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use qframe::runtime::ProcessOutcome;
use qpackages_core::helper::{
    PACMAN_PATH, Refusal, Request, Response, VERSION, build_caches, caller_uid, in_build_cache, pacman_env,
    parse_options, passwd_home,
};
use qpackages_core::pacman::{command, read_orphans};
use qpackages_core::reflector::{BACKUP, CONFIG, MIRRORLIST, Mirrors, REFLECTOR_PATH, has_server};
use qpackages_core::snap;

use crate::runner::{Real, Runner};

/// The user id of root.
const ROOT: u32 = 0;

/// The longest request line read, in bytes: far more than any list of package names needs, and
/// a bound on what a runaway writer can make the helper hold.
const MAX_LINE: usize = 1 << 20;

/// The pseudo-terminal pacman gets before a `size` request says otherwise.
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// The user who started the helper, whose build cache is the only place package files are
/// installed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// Their user id, which every file installed must belong to.
    pub uid: u32,
    /// Their home folder, from the password database.
    pub home: PathBuf,
}

impl Caller {
    /// The user pkexec or sudo named in `lookup`, with their home folder from the password
    /// database under `root`; `None` when either is missing, and then no built package is
    /// installed.
    #[must_use]
    pub fn find(lookup: impl Fn(&str) -> Option<String>, root: &Path) -> Option<Self> {
        let uid = caller_uid(lookup)?;
        let passwd = fs::read_to_string(root.join("etc/passwd")).ok()?;
        let home = passwd_home(&passwd, uid)?;
        Some(Self { uid, home })
    }
}

/// Runs the helper on this process's standard input and output. `args` are the arguments after
/// the helper flag.
///
/// # Errors
///
/// Returns the error of a read from standard input or a write to standard output that failed.
pub fn run(args: &[String]) -> io::Result<()> {
    let root = Path::new("/");
    let caller = Caller::find(|name| std::env::var(name).ok(), root);
    let places = Places { root, caller: caller.as_ref() };
    answer(args, crate::current_uid(), &places, io::stdin().lock(), io::stdout().lock(), &Real)
}

/// Where the helper works: the files it writes itself, such as the mirror list, are under
/// `root` (`/` on a real machine), and built packages come only from `caller`'s build cache.
#[derive(Debug, Clone, Copy)]
pub struct Places<'a> {
    /// The root of the file system.
    pub root: &'a Path,
    /// The user who started the helper, when pkexec or sudo said.
    pub caller: Option<&'a Caller>,
}

/// Runs the helper as user `uid` with arguments `args` at `places`, reading `input` and writing
/// `output`.
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
    places: &Places<'_>,
    input: impl BufRead,
    mut output: impl Write,
    runner: &dyn Runner,
) -> io::Result<()> {
    if uid != Some(ROOT) {
        return respond(&mut output, &Response::Refused(Refusal::NotRoot));
    }
    match parse_options(args) {
        Ok(locale) => serve(places, input, output, runner, &locale),
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
    places: &Places<'_>,
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
            Ok(Request::Mirrors(mirrors)) => choose_mirrors(places.root, &mirrors, &env, size, runner, &mut output),
            Ok(Request::InstallBuilt(paths)) => install_built(&paths, places.caller, &env, size, runner, &mut output),
            Ok(Request::Snap(job, names)) => snap_job(job, &names, &env, runner, &mut output),
            Ok(Request::SnapLink) => snap_link(places.root, &mut output),
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

/// Installs the package files at `paths`, each of them opened and checked first: all of them
/// pass or none is installed.
///
/// pacman is handed `/proc/<helper>/fd/<n>` for each file the helper holds open, so what it
/// reads is the very file checked, whatever happens to the path in between. The helper's own
/// process id rather than pacman's `/proc/self` keeps the descriptors out of pacman and the
/// package scripts it runs: they are opened close-on-exec, and root may open another root
/// process's descriptors through `/proc`.
fn install_built(
    paths: &[String],
    caller: Option<&Caller>,
    env: &[(&str, &str)],
    size: (u16, u16),
    runner: &dyn Runner,
    output: &mut impl Write,
) -> Response {
    let Some(caller) = caller else { return Response::Refused(Refusal::Built) };
    let Ok(files) = paths.iter().map(|path| open_built(Path::new(path), caller)).collect::<Result<Vec<File>, _>>()
    else {
        return Response::Refused(Refusal::Built);
    };
    let pid = std::process::id();
    let held: Vec<String> = files.iter().map(|file| format!("/proc/{pid}/fd/{}", file.as_raw_fd())).collect();
    carry_out(PACMAN_PATH, &command::install_built(&held), env, size, runner, output)
}

/// Opens the package file at `path` when it follows the file rule: below `caller`'s build cache
/// through no symbolic link, a regular file, and theirs. What was looked at through the path is
/// compared with what the open descriptor holds, so a file swapped in between is refused.
fn open_built(path: &Path, caller: &Caller) -> io::Result<File> {
    let refused = || io::Error::from(io::ErrorKind::PermissionDenied);
    if !in_build_cache(path, &caller.home) {
        return Err(refused());
    }
    let seen = fs::symlink_metadata(path)?;
    if !seen.file_type().is_file() {
        return Err(refused());
    }
    // A folder on the way may be a link out of the cache; the resolved path must still be in it.
    let real = fs::canonicalize(path)?;
    let caches: Vec<PathBuf> =
        build_caches(&caller.home).iter().filter_map(|cache| fs::canonicalize(cache).ok()).collect();
    if !caches.iter().any(|cache| real.starts_with(cache) && real != *cache) {
        return Err(refused());
    }
    let file = File::open(&real)?;
    let held = file.metadata()?;
    let same = held.dev() == seen.dev() && held.ino() == seen.ino();
    if !held.file_type().is_file() || !same || held.uid() != caller.uid {
        return Err(refused());
    }
    Ok(file)
}

/// Has snapd carry out `job` on `names` and waits for it to end.
///
/// This is the one request that runs its program twice, and on pipes rather than a pseudo-terminal.
/// `snap install` on a terminal draws a spinner and a progress bar by rewriting one line over and
/// over, which is nothing a pane of lines could show; with `--no-wait` it prints the job's number
/// and lets go instead. The number goes back as [`Response::SnapChange`], and qpac follows the job
/// itself, reading snapd's socket as the user — snapd lets a user watch even a job root started.
/// The helper then waits with `snap watch`, which prints nothing on a pipe, so that the request is
/// only answered once the job is really over.
///
/// snapd's own words still reach the pane: every line the first run printed is passed on, the
/// number excepted. Some of those lines are answers rather than failures — "already installed" and
/// "no updates available" both end with code 0 — which is why the result the screen believes comes
/// from the job's own status, not from this code.
fn snap_job(
    job: qpackages_core::snap::Job,
    names: &[String],
    env: &[(&str, &str)],
    runner: &dyn Runner,
    output: &mut impl Write,
) -> Response {
    let Ok(started) = runner.output(snap::SNAP_PATH, &snap::job_args(job, names), env) else {
        return Response::Refused(Refusal::Start);
    };
    let id = snap::change_id(&started.stdout);
    let number = id.map(|id| id.to_string());
    for line in started.stdout.lines().chain(started.stderr.lines()) {
        // The number is the answer to the request, not something to show.
        if Some(line.trim()) == number.as_deref() {
            continue;
        }
        let _ = respond(output, &Response::Line(line.to_owned()));
    }
    let Some(id) = id else { return Response::Done(started.code) };
    if respond(output, &Response::SnapChange(id)).is_err() {
        return Response::Refused(Refusal::Start);
    }
    match runner.output(snap::SNAP_PATH, &snap::watch_args(id), env) {
        Ok(waited) => {
            for line in waited.stdout.lines().chain(waited.stderr.lines()) {
                let _ = respond(output, &Response::Line(line.to_owned()));
            }
            Response::Done(waited.code)
        }
        Err(_) => Response::Refused(Refusal::Start),
    }
}

/// Makes the link a snap with classic confinement needs: `/snap` pointing at snapd's own folder
/// under `root`.
///
/// Anything already at that path is left exactly as it is, whether it is the link itself or
/// something else the machine's owner put there. Only a path with nothing at it is written, so the
/// request can never replace a folder or send `/snap` somewhere new.
fn snap_link(root: &Path, output: &mut impl Write) -> Response {
    let link = root.join(snap::SNAP_LINK.trim_start_matches('/'));
    let target = root.join(snap::SNAP_DIR.trim_start_matches('/'));
    match fs::read_link(&link) {
        Ok(found) if found == target => return Response::Done(Some(0)),
        // Something else is there: a folder, a file, or a link somewhere else.
        Ok(_) => return Response::Refused(Refusal::SnapLink),
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Response::Refused(Refusal::SnapLink),
        Err(_) => {}
    }
    if fs::symlink_metadata(&link).is_ok() {
        return Response::Refused(Refusal::SnapLink);
    }
    match std::os::unix::fs::symlink(&target, &link) {
        Ok(()) => {
            let _ = respond(output, &Response::Line(format!("{} -> {}", link.display(), target.display())));
            Response::Done(Some(0))
        }
        Err(_) => Response::Refused(Refusal::SnapLink),
    }
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
    use qpackages_core::helper::SYSTEMCTL_PATH;
    use qpackages_core::reflector::Mirrors;

    use super::*;
    use crate::runner::Recorded;

    /// Serves `input` with `recorded` and returns what the helper wrote, line by line.
    fn served(recorded: &Recorded, input: &[u8]) -> Vec<String> {
        let mut output = Vec::new();
        serve(&Places { root: &nowhere(), caller: None }, input, &mut output, recorded, "tr_TR.UTF-8")
            .expect("the helper serves to the end");
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
            answer(
                &args,
                uid,
                &Places { root: &nowhere(), caller: None },
                &b"install cowsay\n"[..],
                &mut output,
                &recorded,
            )
            .expect("answered");
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
    fn a_system_flatpak_is_removed_by_flatpaks_path_and_a_bad_id_runs_nothing() {
        let recorded = Recorded::default();
        let args = qpackages_core::flatpak::system_uninstall_args(&["org.gimp.GIMP".to_owned()]);
        let recording = include_str!("../../../qpackages-core/tests/fixtures/flatpak/uninstall-system.out");
        let lines: Vec<&str> = recording.lines().collect();
        recorded.play("/usr/bin/flatpak", &args, &lines, ProcessOutcome::Finished { code: Some(0) });
        let input = b"flatpak-system remove --delete-data\nflatpak-system install org.gimp.GIMP\n\
            flatpak-system remove org.gimp.GIMP\nflatpak-system remove org.gimp.GIMP";
        let out = served(&recorded, input);
        assert_eq!(
            out,
            [
                "ready 1",
                "refused flatpak-id",
                "refused values",
                "line Uninstalling app/net.sourceforge.ExtremeTuxRacer/x86_64/stable",
                "done 0"
            ],
            "the last request lacks its newline and is not carried out"
        );
        let calls = recorded.calls();
        assert_eq!(calls.len(), 1, "only the valid, whole request ran");
        assert_eq!(calls[0].program, "/usr/bin/flatpak");
        assert_eq!(calls[0].args, ["--system", "uninstall", "--noninteractive", "--", "org.gimp.GIMP"]);
        let env: Vec<(&str, &str)> = calls[0].env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(env, [("PATH", "/usr/bin:/usr/sbin"), ("LANG", "tr_TR.UTF-8"), ("LC_ALL", "tr_TR.UTF-8")]);
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
        serve(&Places { root, caller: None }, input, &mut output, runner, "C").expect("the helper serves to the end");
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
        let result =
            serve(&Places { root: &nowhere(), caller: None }, &b"install cowsay\n"[..], Closing(1), &recorded, "C");
        assert_eq!(result.map_err(|error| error.kind()), Err(io::ErrorKind::BrokenPipe), "the end cannot be said");
        assert_eq!(recorded.calls().len(), 1, "pacman ran to its end all the same");
    }

    /// Plays pacman from `recorded`, and for `pacman -U` reads every file it is handed, as pacman
    /// would, keeping what it read: the paths are the helper's open descriptors, whose numbers a
    /// recording cannot know.
    #[derive(Default)]
    struct Upgrade {
        recorded: Recorded,
        read: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl Runner for Upgrade {
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
            let Some(paths) = args.strip_prefix(&["-U".to_owned(), "--noconfirm".to_owned(), "--".to_owned()]) else {
                return self.recorded.stream(program, args, env, pty, cancel, on_line);
            };
            assert_eq!(program, PACMAN_PATH);
            for path in paths {
                let bytes = fs::read(path)?;
                self.read.lock().expect("not poisoned").push((path.clone(), bytes));
            }
            on_line("loading packages...".to_owned());
            Ok(ProcessOutcome::Finished { code: Some(0) })
        }
    }

    /// A home folder with paru's and yay's build caches, under the system's temporary folder.
    struct Home {
        root: PathBuf,
        caller: Caller,
    }

    impl Home {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("qpackages-helper-built-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let home = root.join("home/ayse");
            for cache in [".cache/paru/clone/hello", ".cache/yay/hello"] {
                fs::create_dir_all(home.join(cache)).expect("a build cache");
            }
            let uid = crate::current_uid().expect("the test's own user id");
            Self { root, caller: Caller { uid, home } }
        }

        /// Writes `bytes` to `relative` under the home folder and returns its absolute path.
        fn file(&self, relative: &str, bytes: &[u8]) -> String {
            let path = self.caller.home.join(relative);
            fs::create_dir_all(path.parent().expect("a folder")).expect("its folder");
            fs::write(&path, bytes).expect("the file");
            path.to_string_lossy().into_owned()
        }

        fn serve(&self, caller: Option<&Caller>, runner: &Upgrade, input: &str) -> Vec<String> {
            let mut output = Vec::new();
            let places = Places { root: &self.root, caller };
            serve(&places, input.as_bytes(), &mut output, runner, "C").expect("the helper serves to the end");
            String::from_utf8(output).expect("responses are text").lines().map(str::to_owned).collect()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const HELLO: &str = ".cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst";

    #[test]
    fn a_built_package_is_handed_to_pacman_as_the_descriptor_the_helper_checked() {
        let home = Home::new("good");
        let paru = home.file(HELLO, b"paru's package");
        let yay = home.file(".cache/yay/hello/hello-debug-1.0-1-x86_64.pkg.tar.xz", b"yay's debug package");
        let runner = Upgrade::default();
        let out = home.serve(Some(&home.caller), &runner, &format!("install-built {paru} {yay}\n"));
        assert_eq!(out, ["ready 1", "line loading packages...", "done 0"]);
        let read = runner.read.lock().expect("not poisoned").clone();
        let pid = std::process::id();
        assert_eq!(read.len(), 2);
        for ((path, bytes), expected) in read.iter().zip([&b"paru's package"[..], b"yay's debug package"]) {
            assert!(path.starts_with(&format!("/proc/{pid}/fd/")), "`{path}` is the helper's descriptor");
            assert_eq!(bytes, expected);
        }
    }

    #[test]
    fn a_built_package_outside_the_rule_runs_nothing() {
        let home = Home::new("refused");
        let good = home.file(HELLO, b"package");
        let elsewhere = home.file("Downloads/hello-1.0-1-x86_64.pkg.tar.zst", b"package");
        let beside = home.file(".cache/paru/hello-1.0-1-x86_64.pkg.tar.zst", b"package");
        // A link inside the cache to a file of the user's outside it.
        let link = home.caller.home.join(".cache/paru/clone/hello/link-1.0-1-x86_64.pkg.tar.zst");
        std::os::unix::fs::symlink(&elsewhere, &link).expect("a link");
        // A folder inside the cache that leads out of it.
        let outside = home.root.join("outside");
        fs::create_dir_all(&outside).expect("a folder outside");
        fs::write(outside.join("hello-1.0-1-x86_64.pkg.tar.zst"), b"package").expect("a file outside");
        std::os::unix::fs::symlink(&outside, home.caller.home.join(".cache/yay/out")).expect("a folder link");
        let through = home.caller.home.join(".cache/yay/out/hello-1.0-1-x86_64.pkg.tar.zst");
        let folder = home.caller.home.join(".cache/yay/hello/folder-1.0-1-x86_64.pkg.tar.zst");
        fs::create_dir_all(&folder).expect("a folder with a package's name");
        let missing = home.caller.home.join(".cache/yay/hello/gone-1.0-1-x86_64.pkg.tar.zst");
        let runner = Upgrade::default();
        for path in [
            elsewhere.clone(),
            beside,
            link.to_string_lossy().into_owned(),
            through.to_string_lossy().into_owned(),
            folder.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
            format!("{good} {elsewhere}"),
        ] {
            let out = home.serve(Some(&home.caller), &runner, &format!("install-built {path}\n"));
            assert_eq!(out, ["ready 1", "refused built"], "`{path}`");
        }
        let spelled = [
            format!(
                "{}/../../../Downloads/hello-1.0-1-x86_64.pkg.tar.zst",
                home.caller.home.join(".cache/yay").display()
            ),
            good.replace(".pkg.tar.zst", ".pkg.tar.gz"),
            ".cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst".to_owned(),
        ];
        for path in spelled {
            let out = home.serve(Some(&home.caller), &runner, &format!("install-built {path}\n"));
            assert_eq!(out, ["ready 1", "refused built"], "`{path}`");
        }
        assert!(runner.read.lock().expect("not poisoned").is_empty(), "pacman never ran");
        assert!(runner.recorded.calls().is_empty());
    }

    #[test]
    fn another_users_file_or_an_unknown_caller_is_refused() {
        let home = Home::new("other");
        let good = home.file(HELLO, b"package");
        let runner = Upgrade::default();
        let someone_else = Caller { uid: home.caller.uid + 1, home: home.caller.home.clone() };
        let line = format!("install-built {good}\n");
        assert_eq!(
            home.serve(Some(&someone_else), &runner, &line),
            ["ready 1", "refused built"],
            "not the caller's file"
        );
        assert_eq!(home.serve(None, &runner, &line), ["ready 1", "refused built"], "no caller, no cache");
        let other_home = Caller { uid: home.caller.uid, home: home.root.join("home/mehmet") };
        assert_eq!(home.serve(Some(&other_home), &runner, &line), ["ready 1", "refused built"], "not their cache");
        assert!(runner.read.lock().expect("not poisoned").is_empty());
    }

    #[test]
    fn the_caller_comes_from_the_variable_pkexec_or_sudo_set_and_the_password_database() {
        let home = Home::new("passwd");
        fs::create_dir_all(home.root.join("etc")).expect("etc");
        let passwd =
            format!("root:x:0:0::/root:/bin/bash\nayse:x:1000:1000::{}:/bin/zsh\n", home.caller.home.display());
        fs::write(home.root.join("etc/passwd"), passwd).expect("the password database");
        let sudo = |name: &str| (name == "SUDO_UID").then(|| "1000".to_owned());
        assert_eq!(Caller::find(sudo, &home.root), Some(Caller { uid: 1000, home: home.caller.home.clone() }));
        let stranger = |name: &str| (name == "PKEXEC_UID").then(|| "4242".to_owned());
        assert_eq!(Caller::find(stranger, &home.root), None, "a user the database does not know");
        assert_eq!(Caller::find(|_| None, &home.root), None, "started by neither");
        assert_eq!(Caller::find(sudo, &home.root.join("nowhere")), None, "no password database");
    }

    /// A folder of a test's own, under which the `/snap` link is made and looked at.
    fn snap_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("qpackages-snap-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("var/lib/snapd")).expect("a scratch folder");
        root
    }

    /// Serves `input` with `runner` under `root`.
    fn serve_under(root: &Path, runner: &Recorded, input: &[u8]) -> Vec<String> {
        let mut output = Vec::new();
        serve(&Places { root, caller: None }, input, &mut output, runner, "C").expect("the helper serves to the end");
        String::from_utf8(output).expect("responses are text").lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_snap_job_is_started_without_waiting_then_waited_for_by_its_number() {
        let recorded = Recorded::default();
        let start = snap::job_args(snap::Job::Install, &["hello-world".to_owned()]);
        // What snapd printed in the container: the job's number, and its reminder about $PATH.
        recorded.answer_full(
            snap::SNAP_PATH,
            &start,
            "10\n",
            "Warning: /var/lib/snapd/snap/bin was not found in your $PATH.\n",
            0,
        );
        recorded.answer_full(snap::SNAP_PATH, &snap::watch_args(10), "", "", 0);
        let out = served(&recorded, b"snap install hello-world\n");
        assert_eq!(
            out,
            [
                "ready 1",
                "line Warning: /var/lib/snapd/snap/bin was not found in your $PATH.",
                "snap-change 10",
                "done 0",
            ],
            "the number is the answer, not a line to show"
        );
        assert_eq!(
            recorded.command_lines(),
            [format!("{} install --no-wait -- hello-world", snap::SNAP_PATH), format!("{} watch 10", snap::SNAP_PATH),]
        );
        assert!(recorded.calls().iter().all(|call| call.pty.is_none()), "snap draws nothing worth a terminal");
    }

    #[test]
    fn a_snap_job_snapd_never_took_ends_at_once_with_what_it_said() {
        let recorded = Recorded::default();
        let start = snap::job_args(snap::Job::Install, &["no-such-snap-qpac-xyz".to_owned()]);
        recorded.answer_full(snap::SNAP_PATH, &start, "", "error: snap \"no-such-snap-qpac-xyz\" not found\n", 1);
        let out = served(&recorded, b"snap install no-such-snap-qpac-xyz\n");
        assert_eq!(
            out,
            ["ready 1", "line error: snap \"no-such-snap-qpac-xyz\" not found", "done 1"],
            "no number, so nothing is waited for"
        );
        assert_eq!(recorded.command_lines().len(), 1, "`snap watch` is never run");
    }

    #[test]
    fn a_refresh_of_every_snap_needs_no_name_and_a_bad_name_runs_nothing() {
        let recorded = Recorded::default();
        let all = snap::job_args(snap::Job::Refresh, &[]);
        recorded.answer_full(snap::SNAP_PATH, &all, "14\n", "", 0);
        recorded.answer_full(snap::SNAP_PATH, &snap::watch_args(14), "", "", 0);
        assert_eq!(served(&recorded, b"snap refresh\n"), ["ready 1", "snap-change 14", "done 0"]);
        let refused = Recorded::default();
        assert_eq!(served(&refused, b"snap install Bad_Name\n"), ["ready 1", "refused snap-name"]);
        assert_eq!(served(&refused, b"snap remove -x\n"), ["ready 1", "refused snap-name"]);
        assert_eq!(served(&refused, b"snap purge hello\n"), ["ready 1", "refused values"]);
        assert_eq!(refused.command_lines(), Vec::<String>::new(), "nothing ran");
    }

    #[test]
    fn the_snap_link_is_made_only_where_nothing_is_in_its_way() {
        let root = snap_root("link");
        let recorded = Recorded::default();
        let link = root.join("snap");
        let target = root.join("var/lib/snapd/snap");
        assert_eq!(serve_under(&root, &recorded, b"snap-link\n").last().expect("an answer"), "done 0");
        assert_eq!(fs::read_link(&link).expect("the link is there"), target);
        // Asking again is no change and no failure: the link is already what it should be.
        assert_eq!(serve_under(&root, &recorded, b"snap-link\n").last().expect("an answer"), "done 0");
        assert_eq!(recorded.command_lines(), Vec::<String>::new(), "no program is run for a link");
    }

    #[test]
    fn something_else_at_the_link_is_left_alone() {
        for (name, put) in [("folder", true), ("file", false)] {
            let root = snap_root(&format!("link-{name}"));
            let link = root.join("snap");
            if put {
                fs::create_dir_all(link.join("someone-elses")).expect("a folder in the way");
            } else {
                fs::write(&link, "not a link").expect("a file in the way");
            }
            let out = serve_under(&root, &Recorded::default(), b"snap-link\n");
            assert_eq!(out.last().expect("an answer"), "refused snap-link", "{name}");
            assert!(fs::symlink_metadata(&link).is_ok(), "what was there is still there");
        }
        // A link pointing somewhere else is not replaced either.
        let root = snap_root("link-elsewhere");
        std::os::unix::fs::symlink(root.join("somewhere"), root.join("snap")).expect("a link in the way");
        let out = serve_under(&root, &Recorded::default(), b"snap-link\n");
        assert_eq!(out.last().expect("an answer"), "refused snap-link");
        assert_eq!(fs::read_link(root.join("snap")).expect("still there"), root.join("somewhere"));
    }

    #[test]
    fn snapds_socket_is_switched_with_systemctl_like_reflectors_timer() {
        let recorded = Recorded::default();
        let args = ["enable", "--now", "--", "snapd.socket"].map(str::to_owned).to_vec();
        recorded.play(SYSTEMCTL_PATH, &args, &["Created symlink"], ProcessOutcome::Finished { code: Some(0) });
        let out = served(&recorded, b"timer on snapd.socket\n");
        assert_eq!(out, ["ready 1", "line Created symlink", "done 0"]);
        assert_eq!(recorded.command_lines(), [format!("{SYSTEMCTL_PATH} enable --now -- snapd.socket")]);
    }
}

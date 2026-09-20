//! The shim, the pipes and the relay together, with the real root-side loop of the helper on a
//! thread: nothing here reaches sudo or pacman.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use qframe::runtime::ProcessOutcome;
use qpackages_core::build::{Built, Expected, Plan};
use qpackages_core::helper::{PACMAN_PATH, Refusal};
use qpackages_core::pacman::{Plan as RepoPlan, Step};

use super::pipes::Pipes;
use super::pipes::tests::base;
use super::relay::{Relay, Setup};
use super::shim::{exchange, run};
use crate::helper::root::Caller;
use crate::helper::session::{InProcess, Session};
use crate::runner::{Output, Recorded, Runner};

/// Plays pacman for the helper: every run is kept as `program args…`, and the files `pacman -U`
/// is handed are read, as pacman would, since their paths are the helper's own descriptors.
#[derive(Default)]
pub struct Pacman {
    runs: Mutex<Vec<String>>,
    read: Mutex<Vec<Vec<u8>>>,
}

impl Pacman {
    /// The runs so far.
    pub fn runs(&self) -> Vec<String> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// What `pacman -U` read, file by file.
    pub fn read(&self) -> Vec<Vec<u8>> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Runner for Pacman {
    fn output(&self, program: &str, args: &[String], env: &[(&str, &str)]) -> io::Result<Output> {
        Recorded::default().output(program, args, env)
    }

    fn stream(
        &self,
        program: &str,
        args: &[String],
        _env: &[(&str, &str)],
        _pty: Option<(u16, u16)>,
        _cancel: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(String),
    ) -> io::Result<ProcessOutcome> {
        assert_eq!(program, PACMAN_PATH, "the helper runs pacman alone");
        if let Some(paths) = args.strip_prefix(&["-U".to_owned(), "--noconfirm".to_owned(), "--".to_owned()]) {
            for path in paths {
                self.read.lock().unwrap_or_else(PoisonError::into_inner).push(fs::read(path)?);
            }
        }
        self.runs.lock().unwrap_or_else(PoisonError::into_inner).push(format!("{program} {}", args.join(" ")));
        on_line(format!("({}) done", args[0]));
        Ok(ProcessOutcome::Finished { code: Some(0) })
    }
}

/// A home folder with a package paru built, and the build that made it: `hello` with `scdoc`
/// from the repositories.
pub struct Home {
    root: PathBuf,
    /// The user building.
    pub caller: Caller,
    /// The package file paru left.
    pub package: String,
}

impl Home {
    pub fn new(name: &str) -> Self {
        let root = base(&format!("home-{name}"));
        let home = root.join("home/builder");
        let package = home.join(".cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst");
        fs::create_dir_all(package.parent().expect("its folder")).expect("the build cache");
        fs::write(&package, b"hello's package").expect("the package file");
        let uid = crate::current_uid().expect("this process's user");
        Self { root, caller: Caller { uid, home }, package: package.to_string_lossy().into_owned() }
    }

    /// Building `hello` from the AUR with `scdoc` from the repositories.
    pub fn plan() -> Plan {
        Plan {
            targets: vec!["hello".to_owned()],
            builds: vec![Built {
                name: "hello".to_owned(),
                base: "hello".to_owned(),
                version: "1.0-1".to_owned(),
                maintainer: Some("someone".to_owned()),
            }],
            repo: RepoPlan {
                steps: vec![Step {
                    repo: Some("extra".to_owned()),
                    name: "scdoc".to_owned(),
                    version: "1.11.3-1".to_owned(),
                    size: Some(1024),
                }],
            },
        }
    }

    /// A relay for this home's build, whose helper plays pacman with `pacman`.
    fn relay(&self, pipes: &Pipes, pacman: &Arc<Pacman>) -> Relay {
        let launcher = InProcess::building(Arc::clone(pacman) as Arc<dyn Runner>, 0, Some(self.caller.clone()));
        let session = Session::new(launcher.start_fn());
        session.start().expect("the helper starts");
        let setup = Setup {
            session,
            expected: Expected::new(&self.caller.home, &Self::plan()),
            size: (100, 30),
            refusals: vec![(Refusal::NotThisBuild, "Not a step of this build.".to_owned())],
        };
        Relay::start(pipes, setup).expect("the relay starts")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn uid() -> u32 {
    crate::current_uid().expect("this process's user")
}

/// Sends `line` through the shim and returns its exit code and what it printed.
fn shim(folder: &Path, line: &str) -> (i32, String) {
    let mut out = Vec::new();
    let code = exchange(folder, line, &mut out).expect("the exchange goes through");
    (code, String::from_utf8(out).expect("text"))
}

#[test]
fn a_package_the_build_made_is_installed_through_the_helper_and_the_shim_ends_as_pacman() {
    let home = Home::new("install");
    let pipes = Pipes::create(&home.root, uid()).expect("the pipes");
    let pacman = Arc::new(Pacman::default());
    let relay = home.relay(&pipes, &pacman);
    let (code, out) = shim(pipes.folder(), &format!("install-built {}", home.package));
    assert_eq!((code, out.as_str()), (0, "(-U) done\n"));
    let pid = std::process::id();
    let runs = pacman.runs();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].starts_with(&format!("/usr/bin/pacman -U --noconfirm -- /proc/{pid}/fd/")), "{runs:?}");
    assert_eq!(pacman.read(), [b"hello's package".to_vec()], "pacman read the very file checked");
    let (code, out) = shim(pipes.folder(), "mark-deps scdoc");
    assert_eq!((code, out.as_str()), (0, "(-D) done\n"), "one shim after another");
    assert_eq!(pacman.runs()[1], "/usr/bin/pacman -D --asdeps -- scdoc");
    relay.stop();
}

#[test]
fn a_request_this_build_would_not_make_is_refused_and_runs_nothing() {
    let home = Home::new("refused");
    let pipes = Pipes::create(&home.root, uid()).expect("the pipes");
    let pacman = Arc::new(Pacman::default());
    let relay = home.relay(&pipes, &pacman);
    for line in
        ["install openssh", "remove glibc", "mark-explicit scdoc", "upgrade", "install-built /tmp/x.pkg.tar.zst"]
    {
        let (code, out) = shim(pipes.folder(), line);
        assert_eq!((code, out.as_str()), (1, "Not a step of this build.\n"), "`{line}`");
    }
    let (code, out) = shim(pipes.folder(), "install --overwrite=*");
    assert_eq!((code, out.as_str()), (1, ""), "a malformed line is refused before the build is asked");
    assert!(pacman.runs().is_empty(), "{:?}", pacman.runs());
    relay.stop();
}

#[test]
fn a_shim_without_a_qpac_to_answer_fails_at_once() {
    let home = Home::new("alone");
    let pipes = Pipes::create(&home.root, uid()).expect("the pipes");
    let mut out = Vec::new();
    let result = exchange(pipes.folder(), "mark-deps scdoc", &mut out).map_err(|error| error.raw_os_error());
    assert_eq!(result, Err(Some(6)), "ENXIO: nobody reads the request pipe");
    let pacman = Arc::new(Pacman::default());
    drop(home.relay(&pipes, &pacman));
    let result = exchange(pipes.folder(), "mark-deps scdoc", &mut out).map_err(|error| error.raw_os_error());
    assert_eq!(result, Err(Some(6)), "nor once the relay stopped");
    assert!(pacman.runs().is_empty());
}

#[test]
fn a_request_whose_shim_went_away_runs_nothing() {
    let home = Home::new("gone");
    let pipes = Pipes::create(&home.root, uid()).expect("the pipes");
    let pacman = Arc::new(Pacman::default());
    let relay = home.relay(&pipes, &pacman);
    // A writer that never opened the answer pipe: nobody would hear how it went.
    fs::write(pipes.request(), "mark-deps scdoc\n").expect("the request is written");
    std::thread::sleep(std::time::Duration::from_millis(100));
    relay.stop();
    assert!(pacman.runs().is_empty(), "{:?}", pacman.runs());
}

#[test]
fn the_shim_keeps_the_permission_warm_and_refuses_what_it_does_not_know_without_any_pipe() {
    let args = |line: &str| -> Vec<String> { line.split(' ').map(str::to_owned).collect() };
    assert_eq!(run(&args("/nowhere -v")), 0);
    assert_eq!(run(&args("/nowhere pacman -Syu")), 1);
    assert_eq!(run(&args("/nowhere pacman -S scdoc")), 1, "a folder that is not a build's");
    assert_eq!(run(&[]), 1);
}

#[test]
fn shims_that_come_at_once_each_get_their_own_whole_answer() {
    let home = Home::new("crowd");
    let pipes = Pipes::create(&home.root, uid()).expect("the pipes");
    let pacman = Arc::new(Pacman::default());
    let relay = home.relay(&pipes, &pacman);
    let folder = pipes.folder().to_path_buf();
    let crowd: Vec<_> = (0..6)
        .map(|index| {
            let folder = folder.clone();
            std::thread::spawn(move || {
                let line = if index % 2 == 0 { "mark-deps scdoc" } else { "remove glibc" };
                (0..8).map(|_| (line, shim(&folder, line))).collect::<Vec<_>>()
            })
        })
        .collect();
    for answers in crowd {
        for (line, answer) in answers.join().expect("the shim thread ends") {
            let wanted = if line == "remove glibc" { (1, "Not a step of this build.\n") } else { (0, "(-D) done\n") };
            assert_eq!((answer.0, answer.1.as_str()), wanted, "`{line}`");
        }
    }
    assert_eq!(pacman.runs().len(), 24, "every expected request ran once");
    relay.stop();
}

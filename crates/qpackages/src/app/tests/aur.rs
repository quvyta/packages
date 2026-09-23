//! Building from the AUR through the screen. Nothing here reaches sudo, paru or pacman: planning
//! is answered from recordings, paru is a double that calls the real shim the way paru does, the
//! shim talks through real pipes to the real relay, and the helper is the real root-side loop on a
//! thread, with a pacman that only keeps what it was asked.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::Harness;
use qframe::runtime::ProcessOutcome;
use qframe::storage::Settings;
use qpackages_core::build::plan::check_args;
use qpackages_core::catalog::aur::info_urls;
use qpackages_core::catalog::net::{CURL, curl_args};
use qpackages_core::pacman::command::{PACMAN, print_install};

use super::env;
use crate::app::{Machine, Msg, Places, Qpackages};
use crate::build::tests::Pacman;
use crate::helper::root::Caller;
use crate::helper::session::InProcess;
use crate::runner::{Output, Recorded, Runner};
use crate::testing::{Scratch, click_last};
use crate::transaction::{self, Action};
use crate::{build, settings};

/// A recorded answer of the AUR, pacman or the machine the fixtures were taken on.
fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/build").join(name);
    fs::read_to_string(path).expect("the fixture is readable")
}

/// The calls paru made to the shim while it built cbonsai, and one a build would never make,
/// each as the arguments after the shim's folder.
fn paru_calls(home: &Path) -> Vec<String> {
    let package = home.join(".cache/paru/clone/cbonsai/cbonsai-1.4.2-1-x86_64.pkg.tar.zst");
    vec![
        "pacman --sync --noconfirm --noconfirm -- extra/scdoc".to_owned(),
        "pacman --database --noconfirm --asdeps -- scdoc".to_owned(),
        format!("pacman --upgrade --noconfirm --noconfirm -- {}", package.display()),
        "pacman --database --noconfirm --asexplicit -- cbonsai".to_owned(),
        "pacman --remove --noconfirm --noconfirm -- scdoc".to_owned(),
        "pacman --sync -y -u --noconfirm --".to_owned(),
        "pacman --sync --noconfirm -- extra/openssh".to_owned(),
    ]
}

/// The recipe the `git` double leaves in the clone: what the review shows before the build. It
/// keeps nothing a rule points at, so this test walks the plain path through the review.
const PKGBUILD: &str = "pkgname=cbonsai\npkgver=1.4.2\npkgrel=1\nsource=('https://example.org/cbonsai-1.4.2.tar.gz')\n\
    sha256sums=('9f2c1d')\nbuild() {\n  make PREFIX=/usr\n}\n";

/// The machine's programs: the planning questions from recordings, and paru as a double.
struct Pretend {
    recorded: Recorded,
    home: PathBuf,
    /// What each call of the shim printed and how it ended, in order.
    shims: Mutex<Vec<(String, i32, String)>>,
}

impl Pretend {
    fn new(home: &Path) -> Arc<Self> {
        let recorded = Recorded::default();
        let url = info_urls(&["cbonsai"]).remove(0);
        recorded.answer(CURL, &curl_args(&url), &fixture("aur-info-cbonsai.json"), 0);
        let deps = ["gcc", "ncurses", "scdoc"].map(str::to_owned);
        recorded.answer(PACMAN, &check_args(&deps), &fixture("pacman-check-cbonsai.txt"), 127);
        recorded.answer(PACMAN, &print_install(&["scdoc"]), &fixture("pacman-print-scdoc.txt"), 0);
        let nothing = info_urls(&["nonesuch"]).remove(0);
        recorded.answer(CURL, &curl_args(&nothing), &fixture("aur-info-none.json"), 0);
        Arc::new(Self { recorded, home: home.to_path_buf(), shims: Mutex::new(Vec::new()) })
    }

    fn shims(&self) -> Vec<(String, i32, String)> {
        self.shims.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Plays paru: checks it was asked for the AUR target with qpac as its sudo, leaves the
    /// package where paru would, and calls the shim for every root step.
    fn paru(&self, args: &[String], on_line: &mut dyn FnMut(String)) -> ProcessOutcome {
        let value = |flag: &str| {
            let at = args.iter().position(|arg| arg == flag).unwrap_or_else(|| panic!("{flag} in {args:?}"));
            args[at + 1].clone()
        };
        assert_eq!(value("--sudo"), "/usr/bin/qpac");
        assert_eq!(args.last().map(String::as_str), Some("cbonsai"));
        let flags = value("--sudoflags");
        let [flag, folder] = flags.split_whitespace().collect::<Vec<_>>()[..] else { panic!("{flags}") };
        assert_eq!(flag, "--elevate-shim");
        let package = self.home.join(".cache/paru/clone/cbonsai/cbonsai-1.4.2-1-x86_64.pkg.tar.zst");
        fs::create_dir_all(package.parent().expect("its folder")).expect("the build cache");
        fs::write(&package, b"cbonsai's package").expect("the package");
        on_line("==> Making package: cbonsai 1.4.2-1".to_owned());
        for call in paru_calls(&self.home) {
            let argv: Vec<String> = std::iter::once(folder).chain(call.split(' ')).map(str::to_owned).collect();
            let mut out = Vec::new();
            let code = build::shim::run_to(&argv, &mut out);
            let out = String::from_utf8(out).expect("text");
            for line in out.lines() {
                on_line(line.to_owned());
            }
            self.shims.lock().unwrap_or_else(PoisonError::into_inner).push((call, code, out));
        }
        ProcessOutcome::Finished { code: Some(0) }
    }
}

impl Runner for Pretend {
    fn output(&self, program: &str, args: &[String], env: &[(&str, &str)]) -> io::Result<Output> {
        self.recorded.output(program, args, env)
    }

    fn output_without(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
        remove: &[&str],
    ) -> io::Result<Output> {
        if program == qpackages_core::review::fetch::GIT {
            return clone(args);
        }
        self.recorded.output_without(program, args, env, remove)
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
        if program == "/usr/bin/paru" {
            return Ok(self.paru(args, on_line));
        }
        self.recorded.stream(program, args, env, pty, cancel, on_line)
    }
}

/// Plays `git`: leaves the recipe where a clone of the AUR would, and nothing else.
fn clone(args: &[String]) -> io::Result<Output> {
    let dir = if args.first().is_some_and(|first| first == "-C") {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from(args.last().expect("the clone's folder"))
    };
    fs::create_dir_all(dir.join(".git"))?;
    fs::write(dir.join("PKGBUILD"), PKGBUILD)?;
    Ok(Output { stdout: String::new(), stderr: String::new(), code: Some(0) })
}

/// The programs of a machine with paru, or with no AUR helper at all.
fn with_paru(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

fn without_helper(program: &str) -> Option<PathBuf> {
    ["pacman", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// The screen and what a test looks at behind it. The scratch machine lives as long as it: a
/// test that leaves it out of its pattern would have the folders removed under the screen.
struct Screen {
    h: Harness<Qpackages>,
    machine: Arc<Pretend>,
    pacman: Arc<Pacman>,
    scratch: Scratch,
}

/// The application on a pretend machine found by `lookup`, in `locale`, run by this test's own
/// user, whose helper plays pacman.
fn screen(name: &str, lookup: fn(&str) -> Option<PathBuf>, locale: &str) -> Screen {
    let scratch = Scratch::new(name, &[]);
    let uid = crate::current_uid().expect("this process's user");
    let machine = Pretend::new(&scratch.home());
    let pacman = Arc::new(Pacman::default());
    let caller = Caller { uid, home: scratch.home() };
    let helper = InProcess::building(Arc::clone(&pacman) as Arc<dyn Runner>, 0, Some(caller));
    let settings = Settings::parse_str("settings.toml", "").schema(settings::schema());
    let app = Qpackages::new(
        Machine {
            dbpath: &scratch.local(),
            sync_dir: &scratch.sync(),
            applications: &scratch.applications(),
            check_dir: None,
            lock_dir: &scratch.lock(),
            lookup: Arc::new(lookup),
            runner: Arc::clone(&machine) as Arc<dyn Runner>,
            helper: helper.start_fn(),
            uid: Some(uid),
            utc_offset: 0,
            app_catalog: &scratch.catalog(),
            flatpak_catalogs: &[],
            appearance: crate::appearance_in(scratch.root()),
            snap_socket: &scratch.root().join("snapd.socket"),
            first_run: None,
        },
        &settings,
    )
    .with_places(Places {
        root: scratch.root().to_path_buf(),
        units: None,
        exe: Some(PathBuf::from("/usr/bin/qpac")),
        runtime: Some(scratch.runtime()),
        home: Some(scratch.home()),
        cache: Some(scratch.root().join("cache")),
        data: Some(scratch.root().join("data")),
    });
    let mut h = Harness::with_env(app, env(), 120, 34);
    h.set_locale(locale).set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    settle(&mut h);
    Screen { h, machine, pacman, scratch }
}

fn settle(h: &mut Harness<Qpackages>) -> String {
    for _ in 0..4 {
        h.advance(Duration::from_millis(20));
    }
    h.screen()
}

fn ask_to_build(h: &mut Harness<Qpackages>) -> String {
    h.send(Msg::Transaction(transaction::Msg::Begin(Action::AurInstall(vec!["cbonsai".to_owned()]))));
    settle(h)
}

/// What the build asked of the helper and what the helper ran, in either language.
fn check_the_run(machine: &Pretend, pacman: &Pacman, refused_text: &str) {
    let shims = machine.shims();
    let codes: Vec<i32> = shims.iter().map(|(_, code, _)| *code).collect();
    assert_eq!(codes, [0, 0, 0, 0, 0, 1, 1], "{shims:#?}");
    assert_eq!(shims[5].2, "", "a system update is refused by the shim itself, before qpac is asked");
    assert_eq!(shims[6].2, format!("{refused_text}\n"), "a package outside the build is refused by qpac");
    let runs = pacman.runs();
    assert_eq!(runs.len(), 5, "{runs:#?}");
    assert_eq!(runs[0], "/usr/bin/pacman -S --needed --noconfirm -- scdoc");
    assert_eq!(runs[1], "/usr/bin/pacman -D --asdeps -- scdoc");
    let pid = std::process::id();
    assert!(runs[2].starts_with(&format!("/usr/bin/pacman -U --noconfirm -- /proc/{pid}/fd/")), "{runs:#?}");
    assert_eq!(runs[3], "/usr/bin/pacman -D --asexplicit -- cbonsai");
    assert_eq!(runs[4], "/usr/bin/pacman -Rns --noconfirm -- scdoc");
    assert_eq!(pacman.read(), [b"cbonsai's package".to_vec()], "pacman read the file paru built");
}

#[test]
fn an_aur_package_is_planned_confirmed_and_built_with_its_root_steps_through_the_helper() {
    let Screen { mut h, machine, pacman, scratch } = screen("aur-en", with_paru, "en");
    let screen = ask_to_build(&mut h);
    for text in [
        "Build 1 package from the AUR?",
        "paru builds these as you, then installs them with",
        "aur/cbonsai  1.4.2-1",
        "built here",
        "extra/scdoc  1.11.5-1",
        "Total download",
        "What is needed only to build them is removed again",
        "Installing a package from the AUR gives its",
        "Administrator permission will be asked;",
        "Cancel",
        "Build and install",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(machine.shims().is_empty() && pacman.runs().is_empty(), "nothing runs before the user confirms");
    click_last(&mut h, "Build and install");
    let reading = settle(&mut h);
    assert!(reading.contains("Read the recipes before they are built"), "the recipe is read first:\n{reading}");
    assert!(h.handoffs().is_empty(), "nothing is asked for while the recipe is being read");
    click_last(&mut h, "Reviewed, install");
    let screen = settle(&mut h);
    assert_eq!(h.handoffs().len(), 1, "sudo asked once, before the helper started");
    check_the_run(
        &machine,
        &pacman,
        "The AUR build asked for something it was not expected to; qpac did not pass it on.",
    );
    assert!(screen.contains("Installed cbonsai"), "the success toast:\n{screen}");
    let folder = scratch.runtime().join("quvyta-packages");
    assert_eq!(fs::read_dir(&folder).map(Iterator::count).ok(), Some(0), "the build's pipes are gone");
}

#[test]
fn turkish_names_the_build_in_its_own_words() {
    let Screen { mut h, machine, pacman, scratch: _scratch } = screen("aur-tr", with_paru, "tr");
    let screen = ask_to_build(&mut h);
    for text in [
        "AUR'dan 1 paket derlensin mi?",
        "paru bunları senin kullanıcınla",
        "aur/cbonsai  1.4.2-1",
        "burada derlenir",
        "Yalnızca derlemek için gerekenler",
        "AUR'dan paket kurmak, paketin bakımcısına",
        "Vazgeç",
        "Derle ve kur",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    click_last(&mut h, "Derle ve kur");
    let reading = settle(&mut h);
    assert!(reading.contains("Derlenmeden önce tarifleri oku"), "{reading}");
    click_last(&mut h, "İncelendi, kur");
    let screen = settle(&mut h);
    check_the_run(&machine, &pacman, "AUR derlemesi beklenmeyen bir şey istedi; qpac bunu iletmedi.");
    assert!(screen.contains("cbonsai kuruldu"), "{screen}");
}

#[test]
fn cancelling_the_confirmation_builds_nothing() {
    let Screen { mut h, machine, pacman, scratch: _scratch } = screen("aur-cancel", with_paru, "en");
    ask_to_build(&mut h);
    click_last(&mut h, "Cancel");
    let screen = settle(&mut h);
    assert!(!screen.contains("Build and install"), "{screen}");
    assert!(h.handoffs().is_empty());
    assert!(machine.shims().is_empty() && pacman.runs().is_empty());
}

#[test]
fn without_paru_or_yay_nothing_is_planned_and_paru_from_source_is_named() {
    let Screen { mut h, machine, scratch: _scratch, .. } = screen("aur-none", without_helper, "en");
    let screen = ask_to_build(&mut h);
    assert!(screen.contains("Building from the AUR needs paru or yay"), "{screen}");
    assert!(screen.contains("rather than paru-bin"), "{screen}");
    let asked = machine.recorded.command_lines();
    assert!(!asked.iter().any(|line| line.contains("-T") || line.contains("cbonsai")), "{asked:#?}");
}

#[test]
fn a_package_the_aur_does_not_have_is_said_and_nothing_is_confirmed() {
    let Screen { mut h, machine, scratch: _scratch, .. } = screen("aur-missing", with_paru, "en");
    h.send(Msg::Transaction(transaction::Msg::Begin(Action::AurInstall(vec!["nonesuch".to_owned()]))));
    let screen = settle(&mut h);
    assert!(screen.contains("The build could not be planned"), "{screen}");
    assert!(screen.contains("nonesuch is not in the AUR."), "{screen}");
    assert!(!screen.contains("Build and install"), "{screen}");
    assert!(machine.shims().is_empty());
}

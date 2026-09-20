//! A pretend machine on disk for the screen tests: a pacman local database with the packages a
//! test asks for, each with its file list, and an application menu folder with their launchers.
//! Everything lives in a folder of its own under the system's temporary place and goes when the
//! test is done.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use qframe::prelude::{App, Harness};
use qframe::storage::Settings;
use qframe::widgets::Appearance;

use crate::app::{Machine, Places, Qpackages};
use crate::helper::session::InProcess;
use crate::runner::{Recorded, Runner};
use crate::settings;

/// The programs of the pretend machine: pacman, paru and fakeroot, neither flatpak nor snap.
pub fn programs(program: &str) -> Option<PathBuf> {
    ["pacman", "paru", "fakeroot"].contains(&program).then(|| Path::new("/usr/bin").join(program))
}

/// The application on `scratch`, following `settings`, running programs from `recorded`, with
/// helpers that run as root and play from the same recording. Times are shown in UTC. It stands
/// on the Installed tab, which the pages these tests look at are reached from; the frame's own
/// tests check that qpac opens on Discover.
pub fn app_in(scratch: &Scratch, settings: Settings, recorded: &Arc<Recorded>) -> Qpackages {
    app_with(scratch, settings, recorded, programs)
}

/// [`app_in`] on a machine whose programs `lookup` finds.
pub fn app_with(
    scratch: &Scratch,
    settings: Settings,
    recorded: &Arc<Recorded>,
    lookup: fn(&str) -> Option<PathBuf>,
) -> Qpackages {
    let machine = Machine {
        dbpath: &scratch.local(),
        sync_dir: &scratch.sync(),
        applications: &scratch.applications(),
        check_dir: Some(&scratch.check()),
        lock_dir: &scratch.lock(),
        lookup: Arc::new(lookup),
        runner: Arc::clone(recorded) as Arc<dyn Runner>,
        // The helper works under the scratch folder: the one request that writes a file, the `/snap`
        // link, then lands there and never anywhere on the real machine.
        helper: InProcess::new(recorded, 0).under(scratch.root()).start_fn(),
        uid: Some(1000),
        utc_offset: 0,
        // Folders that do not exist: Discover shows its starter list and reads nothing of this
        // computer's.
        app_catalog: &scratch.catalog(),
        flatpak_catalogs: &[],
        // The family folder is the machine's own, so no test reads or writes the user's.
        appearance: crate::appearance_in(scratch.root()),
        snap_socket: &scratch.root().join("snapd.socket"),
    };
    let places = Places {
        root: scratch.root().to_path_buf(),
        units: Some(scratch.units()),
        exe: Some(PathBuf::from("/usr/bin/qpac")),
        runtime: Some(scratch.runtime()),
        home: Some(scratch.home()),
        cache: Some(scratch.root().join("cache")),
        data: Some(scratch.root().join("data")),
    };
    Qpackages::new(machine, &settings.schema(settings::schema())).with_places(places).on_tab(crate::app::Tab::Installed)
}

/// [`crate::appearance_in`] over a family folder of its own that is taken away again as soon as it
/// has been read, for a test that never changes an appearance row: nothing is left behind.
pub fn appearance_apart() -> Appearance {
    static TAKEN: AtomicUsize = AtomicUsize::new(0);
    let serial = TAKEN.fetch_add(1, Ordering::Relaxed);
    let folder = std::env::temp_dir().join(format!("qpackages-family-{}-{serial}", std::process::id()));
    let rows = crate::appearance_in(&folder);
    let _ = fs::remove_dir_all(&folder);
    rows
}

/// One package of a pretend machine.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub kib: u64,
    pub explicit: bool,
    /// Whether it puts a launcher in the application menu, named after it.
    pub app: bool,
}

impl Sample {
    /// A package asked for by the user, without a launcher.
    pub const fn new(name: &'static str, version: &'static str, description: &'static str) -> Self {
        Self { name, version, description, kib: 1024, explicit: true, app: false }
    }

    /// The same package with a launcher.
    pub const fn app(self) -> Self {
        Self { app: true, ..self }
    }

    /// The same package, pulled in as a dependency.
    pub const fn dependency(self) -> Self {
        Self { explicit: false, ..self }
    }
}

/// The folders of a pretend machine.
#[derive(Debug)]
pub struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// A machine with `samples` installed.
    pub fn new(name: &str, samples: &[Sample]) -> Self {
        static TAKEN: AtomicUsize = AtomicUsize::new(0);
        let serial = TAKEN.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("qpackages-scratch-{name}-{}-{serial}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let scratch = Self { root };
        for folder in [scratch.local(), scratch.applications(), scratch.sync(), scratch.lock(), scratch.runtime()] {
            fs::create_dir_all(folder).expect("a scratch folder");
        }
        for sample in samples {
            scratch.install(sample);
        }
        scratch
    }

    /// The local database.
    pub fn local(&self) -> PathBuf {
        self.root.join("local")
    }

    /// The application menu's folder.
    pub fn applications(&self) -> PathBuf {
        self.root.join("applications")
    }

    /// The repository databases the update check copies.
    pub fn sync(&self) -> PathBuf {
        self.root.join("sync")
    }

    /// Where the update check keeps its copy.
    pub fn check(&self) -> PathBuf {
        self.root.join("check")
    }

    /// Where the repositories' app catalog would be; never created.
    pub fn catalog(&self) -> PathBuf {
        self.root.join("swcatalog")
    }

    /// The user's systemd unit folder.
    pub fn units(&self) -> PathBuf {
        self.root.join("units")
    }

    /// Where pacman's lock would be.
    pub fn lock(&self) -> PathBuf {
        self.root.join("lock")
    }

    /// The user's runtime folder, where an AUR build's pipes are made.
    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// The user's home folder, whose build cache paru and yay build in.
    pub fn home(&self) -> PathBuf {
        self.root.join("home/builder")
    }

    fn install(&self, sample: &Sample) {
        let record = self.local().join(format!("{}-{}", sample.name, sample.version));
        fs::create_dir_all(&record).expect("a record folder");
        let mut desc = format!(
            "%NAME%\n{}\n\n%VERSION%\n{}\n\n%DESC%\n{}\n\n%SIZE%\n{}\n\n",
            sample.name,
            sample.version,
            sample.description,
            sample.kib * 1024
        );
        if !sample.explicit {
            desc.push_str("%REASON%\n1\n\n");
        }
        fs::write(record.join("desc"), desc).expect("a record");
        let mut files = format!("%FILES%\nusr/\nusr/bin/\nusr/bin/{}\n", sample.name);
        if sample.app {
            let launcher = format!("{}.desktop", sample.name);
            files.push_str(&format!("usr/share/applications/\nusr/share/applications/{launcher}\n"));
            let text = format!("[Desktop Entry]\nType=Application\nName={}\nExec={}\n", sample.name, sample.name);
            fs::write(self.applications().join(launcher), text).expect("a launcher");
        }
        fs::write(record.join("files"), files).expect("a file list");
    }

    /// Writes a repository database for the update check to copy.
    pub fn repository(&self, name: &str) {
        fs::write(self.sync().join(format!("{name}.db")), name).expect("a repository database");
    }

    /// The root of the machine, for a test that needs a path of its own inside it.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Where `label` stands as a whole word in `line`: `Install` in a button, not in `Installed`.
fn word_at(line: &str, label: &str) -> Option<usize> {
    line.match_indices(label)
        .map(|(start, _)| start)
        .find(|&start| !line[start + label.len()..].starts_with(char::is_alphabetic))
}

/// Clicks the last place `label` appears on screen as a word: a dialog's action buttons sit at
/// its bottom, below a title that may carry the same word; a toast's `Installed` is not `Install`.
/// While a dialog is open, only its own lines count, the ones its `▌` edge runs down: the page
/// under it may carry the same word further down.
pub fn click_last<A: App>(h: &mut Harness<A>, label: &str) {
    let screen = h.screen();
    let found: Vec<(usize, &str, usize)> =
        screen.lines().enumerate().filter_map(|(y, line)| word_at(line, label).map(|start| (y, line, start))).collect();
    let in_dialog = |(_, line, start): &&(usize, &str, usize)| line[..*start].contains('▌');
    let (y, line, start) = *found
        .iter()
        .rfind(in_dialog)
        .or_else(|| found.last())
        .unwrap_or_else(|| panic!("`{label}` is not on screen:\n{screen}"));
    // The column, not the character: a Chinese or Japanese label stands two cells wide, so what
    // comes before the button on its row takes more columns than it has characters and counting
    // characters would aim the click at the button beside it.
    let x = qframe::text::width(&line[..start]);
    h.click(i32::from(x), i32::try_from(y).expect("a screen row"));
}

/// A stand-in for snapd on a socket of the test's own.
///
/// It speaks what snapd speaks: one request per connection, answered with the headers snapd's own
/// HTTP/1.0 answer carries and the body registered for that path, then the connection closes.
/// Nothing of the real snapd is involved and no snap is ever run; the bodies are the JSON snapd
/// answered in a throwaway container, recorded under `qpackages-core/tests/fixtures/snap`.
///
/// A path with no answer registered gets `404`, which the reading side takes as snapd saying
/// nothing — so a test says only what it wants said.
pub struct FakeSnapd {
    /// The bodies to answer with, by request path. A test may change them while it runs, the way
    /// the recorded runner's answers change.
    answers: Arc<Mutex<HashMap<String, String>>>,
    /// The paths asked for, in order.
    asked: Arc<Mutex<Vec<String>>>,
    socket: PathBuf,
}

impl FakeSnapd {
    /// Starts listening on `socket`, answering nothing yet.
    pub fn start(socket: &Path) -> Self {
        if let Some(folder) = socket.parent() {
            let _ = fs::create_dir_all(folder);
        }
        let _ = fs::remove_file(socket);
        let listener = UnixListener::bind(socket).expect("the scratch folder takes a socket");
        let answers: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let (mine, seen) = (Arc::clone(&answers), Arc::clone(&asked));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let Ok(copy) = stream.try_clone() else { continue };
                let mut line = String::new();
                if BufRead::read_line(&mut BufReader::new(copy), &mut line).is_err() {
                    continue;
                }
                // `GET <path> HTTP/1.0`.
                let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
                seen.lock().expect("no panic held it").push(path.clone());
                let body = mine.lock().expect("no panic held it").get(&path).cloned();
                let answer = match body {
                    Some(body) => format!(
                        "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    ),
                    None => String::from("HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
                };
                let _ = stream.write_all(answer.as_bytes());
                let _ = stream.flush();
            }
        });
        Self { answers, asked, socket: socket.to_path_buf() }
    }

    /// Answers `path` with `body` from now on, replacing what was registered for it.
    pub fn answer(&self, path: &str, body: &str) {
        self.answers.lock().expect("no panic held it").insert(path.to_owned(), body.to_owned());
    }

    /// The paths asked for so far, in order.
    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("no panic held it").clone()
    }

    /// How many times `path` was asked for.
    pub fn reads(&self, path: &str) -> usize {
        self.asked().iter().filter(|asked| *asked == path).count()
    }
}

impl Drop for FakeSnapd {
    fn drop(&mut self) {
        // The socket goes, so a later state read of the same scratch folder finds nothing there;
        // the thread ends with the process, having nothing left to accept.
        let _ = fs::remove_file(&self.socket);
    }
}

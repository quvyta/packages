//! A pretend machine on disk for the screen tests: a pacman local database with the packages a
//! test asks for, each with its file list, and an application menu folder with their launchers.
//! Everything lives in a folder of its own under the system's temporary place and goes when the
//! test is done.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use qframe::storage::Settings;

use crate::app::{Machine, Qpackages};
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
    let machine = Machine {
        dbpath: &scratch.local(),
        sync_dir: &scratch.sync(),
        applications: &scratch.applications(),
        check_dir: Some(&scratch.check()),
        lock_dir: &scratch.lock(),
        lookup: Arc::new(programs),
        runner: Arc::clone(recorded) as Arc<dyn Runner>,
        helper: InProcess::new(recorded, 0).start_fn(),
        uid: Some(1000),
        utc_offset: 0,
        // Folders that do not exist: Discover shows its starter list and reads nothing of this
        // computer's.
        app_catalog: &scratch.catalog(),
        flatpak_catalogs: &[],
    };
    Qpackages::new(machine, &settings.schema(settings::schema())).on_tab(crate::app::Tab::Installed)
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
        for folder in [scratch.local(), scratch.applications(), scratch.sync(), scratch.lock()] {
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

    /// Where pacman's lock would be.
    pub fn lock(&self) -> PathBuf {
        self.root.join("lock")
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

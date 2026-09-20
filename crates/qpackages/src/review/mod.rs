//! Reading the recipes of an AUR build before anything is built.
//!
//! Building an AUR package runs a script its maintainer wrote, and installing it runs another as
//! root, so the build's confirmation is not the last word: it leads here. The recipes are fetched
//! from the AUR, compared with the ones the user last approved and scanned by
//! [`qpackages_core::review`]'s rules; then this screen shows them, and only "Reviewed, install"
//! starts the build.
//!
//! A build of several package bases is shown as one screen rather than one base after another: the
//! build is one decision and deserves one button, and what a rule points at in the last recipe has
//! to be visible while the first one is being read.

mod view;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use qframe::prelude::*;
use qpackages_core::build::Built;
use qpackages_core::review::fetch::{self, Fetch, FetchError};
use qpackages_core::review::summary::{self, Checked, Status};
use qpackages_core::review::{Change, Finding, Recipe, Store};

use crate::runner::Runner;

pub use view::view;

/// Where the review reads the recipes into and writes what the user approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The cache folder the recipes are cloned into, one folder per base.
    pub cache: PathBuf,
    /// The folder the approved recipes are kept in.
    pub approved: PathBuf,
}

impl Places {
    /// The places of a user whose cache folder is `cache` and data folder is `data`, as the
    /// platform names them; `None` when either is missing, since a review that cannot be
    /// recorded would ask the same questions again after every build.
    #[must_use]
    pub fn new(cache: Option<&Path>, data: Option<&Path>) -> Option<Self> {
        // The fetch makes its own `recipes` folder under the one it is given.
        Some(Self { cache: cache?.join("packages"), approved: data?.join("packages/reviewed") })
    }
}

/// One package base whose recipe is read: what the build plan said about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// The package base, which names the recipe's repository.
    pub base: String,
    /// Its maintainer as the AUR named it while planning; `None` when it is orphaned.
    pub maintainer: Option<String>,
}

impl Wanted {
    /// The bases of `builds`, each once, in the order they are built. One recipe can make
    /// several packages, and it is read once however many of them the build installs.
    #[must_use]
    pub fn of(builds: &[Built]) -> Vec<Self> {
        let mut wanted: Vec<Self> = Vec::new();
        for built in builds {
            if !wanted.iter().any(|other| other.base == built.base) {
                wanted.push(Self { base: built.base.clone(), maintainer: built.maintainer.clone() });
            }
        }
        wanted
    }
}

/// One base's recipe as it was fetched and reviewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// What the plan asked for.
    pub wanted: Wanted,
    /// The recipe the AUR serves now.
    pub recipe: Recipe,
    /// How it compares with the approved one and what the rules point at.
    pub checked: Checked,
}

impl Base {
    /// Whether the user has to read this recipe: it was never approved, or it changed in more
    /// than its version, or a rule points at something.
    #[must_use]
    pub fn needs_reading(&self) -> bool {
        !self.checked.needs_no_reading()
    }

    /// The files of the recipe in the order they are read, the `PKGBUILD` first; empty for a
    /// recipe that needs no reading, which has nothing to show.
    #[must_use]
    pub fn files(&self) -> Vec<&str> {
        if !self.needs_reading() {
            return Vec::new();
        }
        let mut names: Vec<&str> = self.recipe.files.iter().map(|file| file.name.as_str()).collect();
        names.sort_by_key(|&name| (name != "PKGBUILD", name));
        names
    }

    /// How many lines of `file` changed, counted as the status counts them, `None` when nothing
    /// in it did or there is nothing to compare it with.
    #[must_use]
    fn changed_lines(&self, file: &str) -> Option<usize> {
        let Change::Changed(files) = &self.checked.review.change else { return None };
        let diff = files.iter().find(|diff| diff.name == file)?;
        Some(summary::changed_lines(&diff.lines))
    }
}

/// Why no recipe could be read. One base failing stops the review: a build installs all of its
/// packages, so a recipe that cannot be shown cannot be approved either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    /// The base whose recipe could not be read.
    pub base: String,
    /// Which sentence says what happened: `review.trouble.<key>` in the language files.
    pub key: &'static str,
    /// What `git` or the file system said, shown as it came.
    pub detail: String,
}

impl Trouble {
    /// The trouble `error` is for `base`.
    fn of(base: &str, error: &FetchError) -> Self {
        let key = match error {
            FetchError::BadName(_) => "bad-name",
            FetchError::NotFound => "not-found",
            FetchError::Folder(_) => "folder",
            FetchError::Link(_) => "link",
            FetchError::Io(_, _) => "unreadable",
        };
        Self { base: base.to_owned(), key, detail: error.to_string() }
    }
}

/// What the screen has to show.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Progress {
    /// The recipes are being fetched and reviewed.
    Reading,
    /// They are on screen.
    Read(Vec<Base>),
    /// Nothing could be read.
    Failed(Trouble),
}

/// What happens on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// The recipes were fetched and reviewed, or the first one that could not be.
    Read(Result<Vec<Base>, Trouble>),
    /// A file was chosen, by its place among the files that can be opened.
    File(usize),
    /// A finding was chosen, by its place in the list of findings.
    Finding(usize),
    /// "Reviewed, install" was pressed.
    Approve,
    /// The screen was left without approving.
    Cancel,
}

/// What the user does with the screen, which the flow around it carries out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing yet.
    Stay,
    /// Every recipe shown was approved: build.
    Approved,
    /// The build was called off.
    Cancelled,
}

/// The review screen's own state.
#[derive(Debug)]
pub struct Screen {
    /// What the bases are, before anything was fetched.
    wanted: Vec<Wanted>,
    progress: Progress,
    /// The file shown at the right: the base's place and the file's name.
    shown: Option<(usize, String)>,
    /// The finding the user last went to, by its place in [`Screen::findings`].
    at: Option<usize>,
}

impl Screen {
    /// A screen that reads the recipes of `wanted`. Start [`Screen::read`] to fill it.
    #[must_use]
    pub fn new(wanted: Vec<Wanted>) -> Self {
        Self { wanted, progress: Progress::Reading, shown: None, at: None }
    }

    /// Fetches and reviews the recipes in the background.
    #[must_use]
    pub fn read(&self, runner: &Arc<dyn Runner>, places: &Places) -> Command<Msg> {
        let (runner, places, wanted) = (Arc::clone(runner), places.clone(), self.wanted.clone());
        Command::perform(move || Msg::Read(read_all(runner.as_ref(), &places, &wanted)))
    }

    /// Takes in what happened and says what the flow around the screen should do next.
    pub fn update(&mut self, message: Msg) -> Decision {
        match message {
            Msg::Read(Ok(bases)) => {
                self.progress = Progress::Read(bases);
                self.shown = self.first_file();
            }
            Msg::Read(Err(trouble)) => self.progress = Progress::Failed(trouble),
            Msg::File(index) => {
                if let Some(spot) = self.spots().into_iter().nth(index) {
                    self.shown = Some(spot);
                    self.at = None;
                }
            }
            Msg::Finding(index) => {
                if let Some((base, finding)) = self.findings().get(index) {
                    let (base, file) = (*base, finding.file.clone());
                    // A finding about the package rather than one of its lines, such as a new
                    // maintainer, still opens the recipe it is about: its first file.
                    self.shown = self
                        .spots()
                        .into_iter()
                        .find(|spot| *spot == (base, file.clone()))
                        .or_else(|| self.spots().into_iter().find(|(at, _)| *at == base))
                        .or_else(|| self.shown.clone());
                    self.at = Some(index);
                }
            }
            Msg::Approve => return Decision::Approved,
            Msg::Cancel => return Decision::Cancelled,
        }
        Decision::Stay
    }

    /// The bases that were read, empty while the fetch runs or after it failed.
    #[must_use]
    fn bases(&self) -> &[Base] {
        match &self.progress {
            Progress::Read(bases) => bases,
            Progress::Reading | Progress::Failed(_) => &[],
        }
    }

    /// Whether every recipe was read and reviewed, so the screen can be approved.
    #[must_use]
    pub fn is_read(&self) -> bool {
        matches!(self.progress, Progress::Read(_))
    }

    /// Every finding of every base, with the base it belongs to, in build order.
    #[must_use]
    fn findings(&self) -> Vec<(usize, &Finding)> {
        self.bases()
            .iter()
            .enumerate()
            .flat_map(|(index, base)| base.checked.findings.iter().map(move |finding| (index, finding)))
            .collect()
    }

    /// Every file that can be opened, in build order and reading order: the base's place among
    /// the bases and the file's name.
    #[must_use]
    fn spots(&self) -> Vec<(usize, String)> {
        self.bases()
            .iter()
            .enumerate()
            .flat_map(|(index, base)| base.files().into_iter().map(move |name| (index, name.to_owned())))
            .collect()
    }

    /// The first file of the first recipe that has to be read.
    #[must_use]
    fn first_file(&self) -> Option<(usize, String)> {
        self.spots().into_iter().next()
    }

    /// Records every recipe the screen showed as approved, so the next build compares against
    /// this one. Returns what could not be written, each message as the system gave it.
    #[must_use]
    pub fn approve(&self, places: &Places) -> Vec<String> {
        let store = Store::new(&places.approved);
        self.bases()
            .iter()
            .filter_map(|base| {
                let maintainer = base.wanted.maintainer.as_deref();
                store.approve(&base.wanted.base, &base.recipe, maintainer).err().map(|error| error.to_string())
            })
            .collect()
    }
}

/// Fetches and reviews every base's recipe, in order, stopping at the first that cannot be read.
fn read_all(runner: &dyn Runner, places: &Places, wanted: &[Wanted]) -> Result<Vec<Base>, Trouble> {
    let store = Store::new(&places.approved);
    let mut bases = Vec::with_capacity(wanted.len());
    for want in wanted {
        let recipe = fetch_one(runner, &places.cache, &want.base)?;
        let checked = summary::check(&store, &want.base, &recipe, want.maintainer.as_deref());
        bases.push(Base { wanted: want.clone(), recipe, checked });
    }
    Ok(bases)
}

/// Brings one base's recipe into the cache with `git` and reads it.
fn fetch_one(runner: &dyn Runner, cache: &Path, base: &str) -> Result<Recipe, Trouble> {
    let plan = Fetch::new(cache, base).map_err(|error| Trouble::of(base, &error))?;
    for run in &plan.runs {
        let output = runner
            .output_without(fetch::GIT, run, &fetch::GIT_ENV, &fetch::GIT_ENV_REMOVE)
            .map_err(|error| Trouble { base: base.to_owned(), key: "no-git", detail: error.to_string() })?;
        if !output.succeeded() {
            let said = last_line(&output.stderr).unwrap_or_else(|| last_line(&output.stdout).unwrap_or_default());
            return Err(Trouble { base: base.to_owned(), key: "clone-failed", detail: said });
        }
    }
    fetch::read_recipe(&plan.dir).map_err(|error| Trouble::of(base, &error))
}

/// The last line `git` printed that says anything: its own error is the last, the lines before it
/// are progress.
fn last_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty()).map(str::to_owned)
}

/// The words a status is said in: `review.status.<key>`, with `n` for the lines that changed.
#[must_use]
fn status_text(status: Status) -> String {
    match status {
        Status::FirstTime => t!("review.status.first-time"),
        Status::Unchanged => t!("review.status.unchanged"),
        Status::VersionOnly => t!("review.status.version-only"),
        Status::Changed(lines) => t!("review.status.changed", n = lines),
    }
}

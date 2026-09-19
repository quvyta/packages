//! Where the store's data comes from: the AppStream catalogs on disk, and the repositories, the
//! AUR, pkgstats and Flathub through the runner.
//!
//! Every function here blocks and is meant to run as background work; none panics on what it
//! reads. The network is reached only through `curl` run by the runner, with the arguments the
//! core gives, so a test answers from recordings and never goes online.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use qpackages_core::catalog::appstream::{self, Component};
use qpackages_core::catalog::aur::{self, AurError, AurPackage, SearchBy};
use qpackages_core::catalog::merge::{App, RepoPackage, id_key, merge};
use qpackages_core::catalog::popularity::{self, FlathubInstalls, PackageShare};
use qpackages_core::catalog::repo::{self, RepoInfo};
use qpackages_core::catalog::{gzip, net};
use qpackages_core::pacman::command::{PACMAN, parsed_env};

use crate::runner::Runner;

/// Where archlinux-appstream-data puts the repositories' catalogs.
pub const SWCATALOG: &str = "/usr/share/swcatalog/xml";

/// Where Flatpak keeps each remote's catalog for the whole system.
const FLATPAK_SYSTEM: &str = "/var/lib/flatpak/appstream";

/// How many of pkgstats' most used packages are asked for: far more than the applications a
/// "popular" row can show, a fraction of the whole list.
const PKGSTATS_TOP: u32 = 5_000;

/// How many of Flathub's most installed applications are asked for.
const FLATHUB_TOP: u32 = 250;

/// What the store reads and runs.
#[derive(Clone)]
pub struct Machine {
    /// Runs `pacman` and `curl`.
    pub runner: Arc<dyn Runner>,
    /// The folder of the repositories' AppStream catalogs.
    pub swcatalog: PathBuf,
    /// The folders Flatpak keeps its remotes' catalogs in, system-wide and for this user.
    pub flatpak: Vec<PathBuf>,
}

impl fmt::Debug for Machine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Machine")
            .field("swcatalog", &self.swcatalog)
            .field("flatpak", &self.flatpak)
            .finish_non_exhaustive()
    }
}

/// The folders Flatpak keeps its remotes' catalogs in on this machine: the system's, then this
/// user's.
#[must_use]
pub fn flatpak_catalogs() -> Vec<PathBuf> {
    let mut folders = vec![PathBuf::from(FLATPAK_SYSTEM)];
    if let Some(home) = std::env::var_os("HOME") {
        folders.push(Path::new(&home).join(".local/share/flatpak/appstream"));
    }
    folders
}

/// Why a source gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The program could not be run or the server could not be reached.
    Unreachable,
    /// The AUR matched too many packages for the term; more letters narrow it.
    TooMany,
    /// An answer came but could not be read.
    Unreadable,
}

/// The catalogs on disk, read and combined.
#[derive(Debug, Default)]
pub struct Loaded {
    /// The repositories' applications, as their catalog lists them.
    pub repo: Vec<Component>,
    /// Flathub's applications.
    pub flathub: Vec<Component>,
    /// Whether the repositories' catalog is installed at all.
    pub has_repo_catalog: bool,
    /// Both catalogs as one entry per application.
    pub apps: Vec<App>,
    /// Where each id's record is: in `repo` when true, in `flathub` otherwise. The repositories'
    /// record wins, the way the merge lets it lead.
    by_id: HashMap<String, (bool, usize)>,
}

impl Loaded {
    /// The catalog record of the application with AppStream id `id`: the repositories' first,
    /// then Flathub's.
    pub fn component(&self, id: &str) -> Option<&Component> {
        match self.by_id.get(&id_key(id))? {
            (true, index) => self.repo.get(*index),
            (false, index) => self.flathub.get(*index),
        }
    }
}

/// Reads every catalog on `machine`. Missing folders and broken files are not errors: what can
/// be read is used, and a missing repositories' catalog is reported through `has_repo_catalog`.
#[must_use]
pub fn load(machine: &Machine) -> Loaded {
    let repo_files = catalog_files(&machine.swcatalog);
    let has_repo_catalog = !repo_files.is_empty();
    let repo: Vec<Component> = repo_files.iter().flat_map(|path| read_catalog(path)).collect();
    let flathub: Vec<Component> =
        machine.flatpak.iter().flat_map(|root| flatpak_files(root)).flat_map(|path| read_catalog(&path)).collect();
    let apps = merge(&repo, &flathub, &[], &[]);
    let listed = |components: Vec<Component>| -> Vec<Component> {
        components.into_iter().filter(|component| component.kind.is_listed()).collect()
    };
    let (repo, flathub) = (listed(repo), listed(flathub));
    let mut by_id = HashMap::new();
    for (index, component) in flathub.iter().enumerate() {
        by_id.insert(id_key(&component.id), (false, index));
    }
    for (index, component) in repo.iter().enumerate() {
        by_id.insert(id_key(&component.id), (true, index));
    }
    Loaded { repo, flathub, has_repo_catalog, apps, by_id }
}

/// The catalog files in a folder, `.xml.gz` or plain `.xml`, in name order.
fn catalog_files(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            name.ends_with(".xml.gz") || name.ends_with(".xml")
        })
        .collect();
    files.sort();
    files
}

/// Each remote's active catalog under a Flatpak `appstream` folder: `<remote>/<arch>/active/`.
fn flatpak_files(root: &Path) -> Vec<PathBuf> {
    let subfolders = |folder: &Path| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
        let mut folders: Vec<PathBuf> =
            entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        folders.sort();
        folders
    };
    subfolders(root)
        .iter()
        .flat_map(|remote| subfolders(remote))
        .flat_map(|arch| catalog_files(&arch.join("active")))
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("appstream.xml")))
        .collect()
}

/// One catalog file's components; nothing when it cannot be read.
fn read_catalog(path: &Path) -> Vec<Component> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let bytes = if path.extension().is_some_and(|extension| extension == "gz") {
        match gzip::decompress(&bytes) {
            Ok(plain) => plain,
            Err(_) => return Vec::new(),
        }
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(&bytes);
    appstream::parse(&text).0.components
}

/// The body of `url`, fetched by `curl` through the runner.
fn fetch(runner: &dyn Runner, url: &str) -> Result<String, Failure> {
    match runner.output(net::CURL, &net::curl_args(url), &[]) {
        Ok(output) if output.succeeded() => Ok(output.stdout),
        _ => Err(Failure::Unreachable),
    }
}

/// pkgstats' most used packages, most used first.
pub fn pkgstats(runner: &dyn Runner) -> Result<Vec<PackageShare>, Failure> {
    let body = fetch(runner, &popularity::pkgstats_url(PKGSTATS_TOP, 0))?;
    popularity::parse_pkgstats(&body).map_err(|_| Failure::Unreadable)
}

/// Flathub's most installed applications, most installed first.
pub fn flathub_popular(runner: &dyn Runner) -> Result<Vec<FlathubInstalls>, Failure> {
    let body = fetch(runner, &popularity::flathub_popular_url(1, FLATHUB_TOP))?;
    popularity::parse_flathub_popular(&body).map_err(|_| Failure::Unreadable)
}

/// The AUR's full records of `names`, in as few requests as fit.
pub fn aur_info(runner: &dyn Runner, names: &[String]) -> Result<Vec<AurPackage>, Failure> {
    let mut packages = Vec::new();
    for url in aur::info_urls(names) {
        packages.extend(aur_answer(&fetch(runner, &url)?)?);
    }
    Ok(packages)
}

/// The AUR's packages matching `term` by name or description. A term the RPC would refuse as too
/// short finds nothing rather than failing.
pub fn search_aur(runner: &dyn Runner, term: &str) -> Result<Vec<AurPackage>, Failure> {
    let Some(url) = aur::search_url(term, SearchBy::NameDesc) else { return Ok(Vec::new()) };
    aur_answer(&fetch(runner, &url)?)
}

/// Reads an RPC answer, telling "too many results" from every other refusal.
fn aur_answer(body: &str) -> Result<Vec<AurPackage>, Failure> {
    aur::parse_response(body).map_err(|error| match error {
        AurError::Rpc(message) if message.contains("Too many") => Failure::TooMany,
        AurError::Rpc(_) | AurError::Malformed(_) => Failure::Unreadable,
    })
}

/// The repositories' packages matching `term`, from `pacman -Ss`. pacman ends with 1 when
/// nothing matches, which is an answer.
pub fn search_repo(runner: &dyn Runner, term: &str) -> Result<Vec<RepoPackage>, Failure> {
    let Some(args) = repo::search_args(term) else { return Ok(Vec::new()) };
    match runner.output(PACMAN, &args, &parsed_env()) {
        Ok(output) if output.succeeded() => Ok(repo::parse_search(&output.stdout)),
        Ok(output) if output.code == Some(1) && output.stdout.trim().is_empty() => Ok(Vec::new()),
        _ => Err(Failure::Unreachable),
    }
}

/// What `pacman -Si` says about the repository package `name`.
pub fn repo_info(runner: &dyn Runner, name: &str) -> Result<RepoInfo, Failure> {
    match runner.output(PACMAN, &repo::info_args(name), &parsed_env()) {
        Ok(output) if output.succeeded() => repo::parse_info(&output.stdout).ok_or(Failure::Unreadable),
        _ => Err(Failure::Unreachable),
    }
}

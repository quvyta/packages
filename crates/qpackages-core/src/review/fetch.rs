//! Fetching a recipe from the AUR to review it before it is built.
//!
//! Every AUR package base is a git repository at `https://aur.archlinux.org/<base>.git`. It is
//! cloned shallowly into the user's cache, once per base, and brought up to date on the next
//! review. Like the catalog's `curl`, `git` is run by the application: the core builds the
//! argument lists and reads what the clone left on disk.
//!
//! Every file of the clone is part of the recipe, not only the ones the `PKGBUILD` names: a
//! source written as `"$_name.service"` is easy to miss when reading names, and a file that is
//! not read cannot be reviewed. Only `.SRCINFO` and the `.git` folder are left out; even a
//! `.gitignore` can be named as a source.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{Recipe, RecipeFile, sha256};
use crate::helper::is_package_name;

/// The program to run.
pub const GIT: &str = "git";

/// The environment `git` runs with: it never stops to ask for a password on a terminal it does
/// not own. The AUR answers a base it does not have with an empty repository, not a login.
pub const GIT_ENV: [(&str, &str); 1] = [("GIT_TERMINAL_PROMPT", "0")];

/// The variables to take out of `git`'s environment. Each points `git` at a repository other
/// than the one its arguments name: run from a hook or a shell inside another repository, they
/// would send the clone, the fetch and the clean to that repository instead.
pub const GIT_ENV_REMOVE: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// Where the AUR's git repositories live.
const AUR: &str = "https://aur.archlinux.org";

/// The folder under the cache that holds one clone per base.
const RECIPES: &str = "recipes";

/// Why a recipe could not be fetched or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The base does not follow pacman's rule for names, so no URL or folder is built from it.
    BadName(String),
    /// The clone has no `PKGBUILD`: the AUR has no such base and served an empty repository.
    NotFound,
    /// The clone holds a folder. The AUR refuses them, so a clone that has one was not made by
    /// the AUR, and its contents would not be reviewed.
    Folder(String),
    /// The clone holds a symbolic link, whose target the review cannot show.
    Link(String),
    /// A file or folder could not be read or written; the path and the system's message.
    Io(PathBuf, String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadName(name) => write!(formatter, "`{name}` is not a package name"),
            Self::NotFound => formatter.write_str("the AUR has no recipe by that name"),
            Self::Folder(name) => write!(formatter, "the recipe holds a folder, `{name}`"),
            Self::Link(name) => write!(formatter, "the recipe holds a symbolic link, `{name}`"),
            Self::Io(path, message) => write!(formatter, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for FetchError {}

/// The URL of `base`'s recipe repository.
///
/// # Errors
///
/// [`FetchError::BadName`] when `base` does not follow pacman's rule for names.
pub fn recipe_url(base: &str) -> Result<String, FetchError> {
    if !is_package_name(base) {
        return Err(FetchError::BadName(base.to_owned()));
    }
    Ok(format!("{AUR}/{base}.git"))
}

/// How to bring one base's recipe into the cache: the folder it lands in and the `git` runs
/// that put it there, each an argument list without the program name, run in order until one
/// fails, with [`GIT_ENV`] set and [`GIT_ENV_REMOVE`] taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    /// The clone's folder; read it with [`read_recipe`] once every run succeeded.
    pub dir: PathBuf,
    /// The runs.
    pub runs: Vec<Vec<String>>,
}

impl Fetch {
    /// The runs that bring `base`'s recipe into `<cache>/recipes/<base>`.
    ///
    /// A folder already there that is a clone is updated in place: the latest commit is fetched
    /// and checked out, and anything else in the folder is removed, so the recipe read afterwards
    /// is exactly the one the AUR serves. A folder there that is not a clone, left by an
    /// interrupted run, is removed here and cloned again.
    ///
    /// # Errors
    ///
    /// [`FetchError::BadName`] for a base that is not a package name, and [`FetchError::Io`]
    /// when the cache folder cannot be made or a leftover folder cannot be removed.
    pub fn new(cache: &Path, base: &str) -> Result<Self, FetchError> {
        let url = recipe_url(base)?;
        Self::from_url(&url, &cache.join(RECIPES).join(base))
    }

    /// The runs that bring the repository at `url` into `dir`.
    fn from_url(url: &str, dir: &Path) -> Result<Self, FetchError> {
        let failed = |path: &Path, error: &io::Error| FetchError::Io(path.to_path_buf(), error.to_string());
        let parent = dir.parent().unwrap_or(dir);
        fs::create_dir_all(parent).map_err(|error| failed(parent, &error))?;
        let arguments = |list: &[&str]| list.iter().map(|&argument| argument.to_owned()).collect::<Vec<String>>();
        let path = dir.to_string_lossy();
        if dir.join(".git").is_dir() {
            // The URL is given again rather than read from the clone's `origin`, so a clone whose
            // configuration was changed still fetches from the AUR.
            return Ok(Self {
                dir: dir.to_path_buf(),
                runs: vec![
                    arguments(&["-C", &path, "fetch", "--depth", "1", "--no-tags", "--quiet", "--", url, "HEAD"]),
                    arguments(&["-C", &path, "reset", "--hard", "--quiet", "FETCH_HEAD"]),
                    arguments(&["-C", &path, "clean", "-d", "-f", "-f", "-x", "--quiet"]),
                ],
            });
        }
        match fs::symlink_metadata(dir) {
            Ok(_) => remove(dir).map_err(|error| failed(dir, &error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(failed(dir, &error)),
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            runs: vec![arguments(&["clone", "--depth", "1", "--no-tags", "--quiet", "--", url, &path])],
        })
    }
}

/// Removes `path`, a folder or anything else.
fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// Reads the recipe a fetch left in `dir`: every file but `.SRCINFO`, in name order.
///
/// A file that is not UTF-8 text, such as an icon, stands in the recipe as `sha256:` and the
/// digest of its bytes, so a changed one still changes the recipe.
///
/// # Errors
///
/// [`FetchError::NotFound`] when there is no `PKGBUILD`, [`FetchError::Folder`] and
/// [`FetchError::Link`] for what the review cannot show, and [`FetchError::Io`] for a read that
/// failed.
pub fn read_recipe(dir: &Path) -> Result<Recipe, FetchError> {
    let failed = |path: &Path, error: &io::Error| FetchError::Io(path.to_path_buf(), error.to_string());
    let entries = fs::read_dir(dir).map_err(|error| failed(dir, &error))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| failed(dir, &error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".SRCINFO" || name == ".git" {
            continue;
        }
        let path = entry.path();
        let kind = fs::symlink_metadata(&path).map_err(|error| failed(&path, &error))?.file_type();
        if kind.is_symlink() {
            return Err(FetchError::Link(name));
        }
        if kind.is_dir() {
            return Err(FetchError::Folder(name));
        }
        let bytes = fs::read(&path).map_err(|error| failed(&path, &error))?;
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|error| format!("sha256:{}\n", sha256::hex_digest(error.as_bytes())));
        files.push(RecipeFile { name, text });
    }
    if !files.iter().any(|file| file.name == "PKGBUILD") {
        return Err(FetchError::NotFound);
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Recipe { files })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    const PKGBUILD: &str = "pkgname=hello\npkgver=2.12\npkgrel=1\ninstall=hello.install\n\
        source=('https://ftp.gnu.org/gnu/hello/hello-2.12.tar.gz' 'hello.service')\n\
        sha256sums=('aaaa' 'bbbb')\npackage() {\n  install -Dm644 hello.service \"$pkgdir/usr/lib/systemd/system/hello.service\"\n}\n";
    const INSTALL: &str = "post_install() {\n  echo 'run hello --setup'\n}\n";
    const SERVICE: &str = "[Service]\nExecStart=/usr/bin/hello\n";

    /// A fresh folder under the system's temporary folder; never the user's own data.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-fetch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("made");
        dir
    }

    /// Runs `git` apart from the machine's own configuration, so a signing key or a template
    /// folder there changes nothing, and apart from every `GIT_` variable of the environment:
    /// the commit gate runs these tests from a hook, whose `GIT_DIR` and `GIT_INDEX_FILE` would
    /// otherwise turn the fixtures' commits into commits of the repository being committed to.
    fn git(arguments: &[&str]) {
        let mut command = Command::new(GIT);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        let status = command
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
            .args(arguments)
            .envs(GIT_ENV)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {arguments:?}");
    }

    fn run(fetch: &Fetch) {
        for arguments in &fetch.runs {
            let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
            git(&arguments);
        }
    }

    /// A repository standing in for the AUR's, holding `files`, one commit per call.
    fn commit(repo: &Path, files: &[(&str, &[u8])]) {
        if !repo.join(".git").exists() {
            git(&["init", "--quiet", "--initial-branch=master", &repo.to_string_lossy()]);
        }
        for (name, bytes) in files {
            fs::write(repo.join(name), bytes).expect("written");
        }
        let path = repo.to_string_lossy();
        git(&["-C", &path, "add", "--all"]);
        git(&["-C", &path, "commit", "--quiet", "--message", "update"]);
    }

    fn url(repo: &Path) -> String {
        format!("file://{}", repo.display())
    }

    fn names(recipe: &Recipe) -> Vec<&str> {
        recipe.files.iter().map(|file| file.name.as_str()).collect()
    }

    #[test]
    fn the_variables_that_redirect_git_are_removed() {
        for name in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
            assert!(GIT_ENV_REMOVE.contains(&name), "{name}");
        }
    }

    #[test]
    fn the_url_is_built_only_from_a_package_name() {
        assert_eq!(recipe_url("hello").as_deref(), Ok("https://aur.archlinux.org/hello.git"));
        for bad in ["", "-u", "--upload-pack=x", ".hidden", "a/b", "../x", "Hello", "a b"] {
            assert_eq!(recipe_url(bad), Err(FetchError::BadName(bad.to_owned())), "`{bad}`");
        }
        let cache = scratch("bad-name");
        assert_eq!(Fetch::new(&cache, "../x"), Err(FetchError::BadName("../x".to_owned())));
        assert!(!cache.join(RECIPES).exists(), "nothing is made for a bad name");
        fs::remove_dir_all(cache).ok();
    }

    #[test]
    fn a_first_fetch_clones_shallowly_into_the_cache() {
        let cache = scratch("first");
        let fetch = Fetch::new(&cache, "hello").expect("planned");
        let dir = cache.join("recipes/hello");
        assert_eq!(fetch.dir, dir);
        assert_eq!(
            fetch.runs,
            [[
                "clone",
                "--depth",
                "1",
                "--no-tags",
                "--quiet",
                "--",
                "https://aur.archlinux.org/hello.git",
                &dir.to_string_lossy()
            ]]
        );
        fs::remove_dir_all(cache).ok();
    }

    #[test]
    fn a_fetched_recipe_holds_its_install_script_and_local_sources() {
        let root = scratch("read");
        let repo = root.join("aur/hello");
        fs::create_dir_all(&repo).expect("made");
        commit(
            &repo,
            &[
                ("PKGBUILD", PKGBUILD.as_bytes()),
                ("hello.install", INSTALL.as_bytes()),
                ("hello.service", SERVICE.as_bytes()),
                (".SRCINFO", b"pkgbase = hello\n"),
                (".gitignore", b"*.tar.gz\n"),
            ],
        );
        let fetch = Fetch::from_url(&url(&repo), &root.join("cache/recipes/hello")).expect("planned");
        run(&fetch);
        let recipe = read_recipe(&fetch.dir).expect("read");
        assert_eq!(names(&recipe), [".gitignore", "PKGBUILD", "hello.install", "hello.service"]);
        assert_eq!(recipe.files[2].text, INSTALL);
        assert_eq!(recipe.files[3].text, SERVICE);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_second_fetch_brings_the_clone_up_to_date_and_drops_leftovers() {
        let root = scratch("refresh");
        let repo = root.join("aur/hello");
        fs::create_dir_all(&repo).expect("made");
        commit(&repo, &[("PKGBUILD", PKGBUILD.as_bytes()), ("hello.install", INSTALL.as_bytes())]);
        let dir = root.join("cache/recipes/hello");
        run(&Fetch::from_url(&url(&repo), &dir).expect("planned"));
        let newer = PKGBUILD.replace("pkgver=2.12", "pkgver=2.13");
        commit(&repo, &[("PKGBUILD", newer.as_bytes())]);
        fs::write(dir.join("hello-2.12.tar.gz"), b"left by a build").expect("written");
        fs::write(dir.join("hello.install"), "post_install() {\n  edited\n}\n").expect("written");

        let fetch = Fetch::from_url(&url(&repo), &dir).expect("planned");
        assert_eq!(fetch.runs.len(), 3, "fetch, check out, clean: {:?}", fetch.runs);
        assert_eq!(fetch.runs[0][..3], ["-C".to_owned(), dir.to_string_lossy().into_owned(), "fetch".to_owned()]);
        run(&fetch);
        let recipe = read_recipe(&dir).expect("read");
        assert_eq!(names(&recipe), ["PKGBUILD", "hello.install"]);
        assert_eq!(recipe.files[0].text, newer);
        assert_eq!(recipe.files[1].text, INSTALL, "a local edit is undone");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_leftover_folder_that_is_not_a_clone_is_replaced() {
        let root = scratch("leftover");
        let dir = root.join("recipes/hello");
        fs::create_dir_all(&dir).expect("made");
        fs::write(dir.join("PKGBUILD"), "half written").expect("written");
        let fetch = Fetch::new(&root, "hello").expect("planned");
        assert_eq!(fetch.runs[0][0], "clone");
        assert!(!dir.exists(), "cleared for the clone");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_base_the_aur_does_not_have_is_not_found() {
        let root = scratch("missing");
        let repo = root.join("aur/nothing");
        git(&["init", "--quiet", "--bare", &repo.to_string_lossy()]);
        let fetch = Fetch::from_url(&url(&repo), &root.join("cache/recipes/nothing")).expect("planned");
        run(&fetch);
        assert_eq!(read_recipe(&fetch.dir), Err(FetchError::NotFound));
        assert!(matches!(read_recipe(&root.join("absent")), Err(FetchError::Io(..))));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_binary_file_stands_as_its_digest_and_folders_and_links_are_refused() {
        let root = scratch("odd");
        fs::write(root.join("PKGBUILD"), PKGBUILD).expect("written");
        fs::write(root.join("icon.png"), [0x89, b'P', b'N', b'G', 0xff, 0x00]).expect("written");
        let recipe = read_recipe(&root).expect("read");
        let icon = &recipe.files[1];
        assert_eq!(icon.name, "icon.png");
        assert_eq!(icon.text, format!("sha256:{}\n", sha256::hex_digest(&[0x89, b'P', b'N', b'G', 0xff, 0x00])));

        fs::create_dir(root.join("patches")).expect("made");
        assert_eq!(read_recipe(&root), Err(FetchError::Folder("patches".to_owned())));
        fs::remove_dir(root.join("patches")).expect("removed");
        std::os::unix::fs::symlink("/etc/hostname", root.join("hostname")).expect("linked");
        assert_eq!(read_recipe(&root), Err(FetchError::Link("hostname".to_owned())));
        fs::remove_dir_all(root).ok();
    }
}

//! Where the approved recipes are kept.
//!
//! One folder per package under the store's folder (the application uses
//! `~/.local/share/quvyta/packages/reviewed/`):
//!
//! ```text
//! reviewed/<package>/record       digest, maintainer and source hosts, one per line
//! reviewed/<package>/files/<name> the approved recipe's files, to compare the next one with
//! ```
//!
//! A record is read back only when the files still hash to its digest, so a copy that was
//! damaged or edited by hand counts as never approved and the recipe is read again in full.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{Recipe, RecipeFile};
use crate::helper::is_package_name;

/// The record file inside a package's folder.
const RECORD: &str = "record";

/// The folder of copied files inside a package's folder.
const FILES: &str = "files";

/// What the user approved for one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approved {
    /// The recipe's SHA-256 digest.
    pub digest: String,
    /// Its maintainer at the time; `None` when it was orphaned.
    pub maintainer: Option<String>,
    /// The hosts its sources downloaded from.
    pub domains: Vec<String>,
    /// The recipe itself.
    pub recipe: Recipe,
}

/// Something in the store that could not be read, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The file.
    pub file: PathBuf,
    /// The line, counted from 1.
    pub line: usize,
    /// The column in characters, counted from 1.
    pub column: usize,
    /// What is wrong, in English: these reach logs, not the screen.
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}:{}: {}", self.file.display(), self.line, self.column, self.message)
    }
}

/// The approved recipes under one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The store in `dir`, which is created on the first approval.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// What was approved for `package`: `None` when nothing was, or when the record cannot be
    /// trusted, and then the diagnostics say why. Never panics on a damaged store.
    #[must_use]
    pub fn load(&self, package: &str) -> (Option<Approved>, Vec<Diagnostic>) {
        let folder = self.dir.join(package);
        let record_path = folder.join(RECORD);
        let at_start =
            |file: &Path, message: String| Diagnostic { file: file.to_path_buf(), line: 1, column: 1, message };
        if !is_package_name(package) {
            return (None, vec![at_start(&folder, format!("`{package}` is not a package name"))]);
        }
        let text = match fs::read_to_string(&record_path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return (None, Vec::new()),
            Err(error) => return (None, vec![at_start(&record_path, error.to_string())]),
        };
        let (fields, mut diagnostics) = read_record(&record_path, &text);
        let Some(digest) = fields.digest else {
            diagnostics.push(at_start(&record_path, "the record has no digest".to_owned()));
            return (None, diagnostics);
        };
        let recipe = match read_files(&folder.join(FILES)) {
            Ok(recipe) => recipe,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return (None, diagnostics);
            }
        };
        if recipe.digest() != digest {
            diagnostics.push(at_start(&record_path, "the kept files no longer match the approved digest".to_owned()));
            return (None, diagnostics);
        }
        (Some(Approved { digest, maintainer: fields.maintainer, domains: fields.domains, recipe }), diagnostics)
    }

    /// Records `recipe`, kept by `maintainer`, as approved for `package`, replacing what was
    /// approved before.
    ///
    /// The new folder is written beside the old one and moved into place only once complete, so
    /// an interrupted approval leaves the earlier one readable.
    ///
    /// # Errors
    ///
    /// Returns an error of kind [`io::ErrorKind::InvalidInput`] for a package or file name that
    /// could reach outside the store, or a maintainer that spans lines; otherwise the error of a
    /// write that failed.
    pub fn approve(&self, package: &str, recipe: &Recipe, maintainer: Option<&str>) -> io::Result<()> {
        let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
        if !is_package_name(package) {
            return Err(invalid(format!("`{package}` is not a package name")));
        }
        if let Some(file) = recipe.files.iter().find(|file| !is_file_name(&file.name)) {
            return Err(invalid(format!("`{}` is not a plain file name", file.name)));
        }
        if maintainer.is_some_and(|name| name.is_empty() || name.contains(['\n', '\r'])) {
            return Err(invalid("the maintainer's name is not one line".to_owned()));
        }
        fs::create_dir_all(&self.dir)?;
        let fresh = self.dir.join(format!(".{package}.new"));
        if fresh.exists() {
            fs::remove_dir_all(&fresh)?;
        }
        fs::create_dir_all(fresh.join(FILES))?;
        for file in &recipe.files {
            fs::write(fresh.join(FILES).join(&file.name), &file.text)?;
        }
        let mut record = format!("digest {}\n", recipe.digest());
        if let Some(name) = maintainer {
            record.push_str(&format!("maintainer {name}\n"));
        }
        for domain in recipe.domains() {
            record.push_str(&format!("domain {domain}\n"));
        }
        fs::write(fresh.join(RECORD), record)?;
        let folder = self.dir.join(package);
        if folder.exists() {
            fs::remove_dir_all(&folder)?;
        }
        fs::rename(fresh, folder)
    }
}

/// The fields of a record.
#[derive(Debug, Default)]
struct Fields {
    digest: Option<String>,
    maintainer: Option<String>,
    domains: Vec<String>,
}

/// Reads a record's lines; a line that does not read is reported where it breaks and skipped.
fn read_record(path: &Path, text: &str) -> (Fields, Vec<Diagnostic>) {
    let mut fields = Fields::default();
    let mut diagnostics = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let problem = |column: usize, message: &str| Diagnostic {
            file: path.to_path_buf(),
            line: index + 1,
            column,
            message: message.to_owned(),
        };
        let Some((key, value)) = line.split_once(' ') else {
            diagnostics.push(problem(line.chars().count() + 1, "a key without a value"));
            continue;
        };
        let value_column = key.chars().count() + 2;
        match key {
            "digest" if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) => {
                fields.digest = Some(value.to_ascii_lowercase());
            }
            "digest" => diagnostics.push(problem(value_column, "a digest is 64 hexadecimal digits")),
            "maintainer" if !value.is_empty() => fields.maintainer = Some(value.to_owned()),
            "domain" if !value.is_empty() && !value.contains(' ') => fields.domains.push(value.to_owned()),
            "maintainer" | "domain" => diagnostics.push(problem(value_column, "an empty or broken value")),
            _ => diagnostics.push(problem(1, &format!("unknown key `{key}`"))),
        }
    }
    (fields, diagnostics)
}

/// Reads the kept files of a recipe.
fn read_files(folder: &Path) -> Result<Recipe, Diagnostic> {
    let fail = |file: &Path, error: &io::Error| Diagnostic {
        file: file.to_path_buf(),
        line: 1,
        column: 1,
        message: error.to_string(),
    };
    let entries = fs::read_dir(folder).map_err(|error| fail(folder, &error))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| fail(folder, &error))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).map_err(|error| fail(&path, &error))?;
        files.push(RecipeFile { name, text });
    }
    Ok(Recipe { files })
}

/// A name that stays inside the folder it is joined to: not empty, not `.` or `..`, no `/` and
/// no NUL.
fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh store under the system's temporary folder; never the user's own data.
    fn store(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("qpackages-reviewed-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (Store::new(&dir), dir)
    }

    fn recipe() -> Recipe {
        Recipe {
            files: vec![
                RecipeFile {
                    name: "PKGBUILD".to_owned(),
                    text: "pkgname=hello\nsource=('https://ftp.gnu.org/a.tar.gz')\n".to_owned(),
                },
                RecipeFile { name: "hello.install".to_owned(), text: "post_install() {\n  true\n}\n".to_owned() },
            ],
        }
    }

    #[test]
    fn an_approval_reads_back_whole() {
        let (store, dir) = store("round-trip");
        assert_eq!(store.load("hello"), (None, Vec::new()), "nothing approved yet");
        store.approve("hello", &recipe(), Some("someone")).expect("written");
        let (approved, diagnostics) = store.load("hello");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let approved = approved.expect("approved");
        assert_eq!(approved.digest, recipe().digest());
        assert_eq!(approved.maintainer.as_deref(), Some("someone"));
        assert_eq!(approved.domains, ["ftp.gnu.org"]);
        let mut names: Vec<String> = approved.recipe.files.iter().map(|file| file.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["PKGBUILD", "hello.install"]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_new_approval_replaces_the_old_one_entirely() {
        let (store, dir) = store("replace");
        store.approve("hello", &recipe(), Some("someone")).expect("written");
        let smaller = Recipe { files: vec![recipe().files[0].clone()] };
        store.approve("hello", &smaller, None).expect("written again");
        let (approved, _) = store.load("hello");
        let approved = approved.expect("approved");
        assert_eq!(approved.recipe.files.len(), 1, "the dropped install script is gone");
        assert_eq!(approved.maintainer, None);
        assert!(!dir.join(".hello.new").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn names_that_could_leave_the_store_are_refused() {
        let (store, dir) = store("names");
        for package in ["../x", "", "Hello", "a/b"] {
            let error = store.approve(package, &recipe(), None).expect_err("refused");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "`{package}`");
        }
        for name in ["..", ".", "", "a/b", "../../x"] {
            let bad = Recipe { files: vec![RecipeFile { name: name.to_owned(), text: String::new() }] };
            let error = store.approve("hello", &bad, None).expect_err("refused");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "`{name}`");
        }
        let error = store.approve("hello", &recipe(), Some("a\nb")).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!dir.exists(), "nothing was written");
        let (approved, diagnostics) = store.load("../etc");
        assert_eq!(approved, None);
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn edited_files_are_no_longer_approved() {
        let (store, dir) = store("edited");
        store.approve("hello", &recipe(), None).expect("written");
        fs::write(dir.join("hello/files/PKGBUILD"), "pkgname=evil\n").expect("edited");
        let (approved, diagnostics) = store.load("hello");
        assert_eq!(approved, None);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].to_string().ends_with("record:1:1: the kept files no longer match the approved digest"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_broken_record_says_where_and_never_panics() {
        let (store, dir) = store("broken");
        store.approve("hello", &recipe(), Some("someone")).expect("written");
        let record = dir.join("hello/record");
        let digest = recipe().digest();
        fs::write(&record, format!("digest {digest}\ncolour blue\nmaintainer\ndomain \ndigest xyz\n"))
            .expect("written");
        let (approved, diagnostics) = store.load("hello");
        let places: Vec<(usize, usize)> = diagnostics.iter().map(|d| (d.line, d.column)).collect();
        assert_eq!(places, [(2, 1), (3, 11), (4, 8), (5, 8)]);
        assert!(approved.is_some(), "the first digest still stands");
        fs::write(&record, "maintainer someone\n").expect("written");
        let (approved, diagnostics) = store.load("hello");
        assert_eq!(approved, None);
        assert_eq!(diagnostics[0].message, "the record has no digest");
        fs::write(&record, [0xff, 0xfe]).expect("written");
        assert_eq!(store.load("hello").0, None, "a record that is not text");
        fs::remove_dir_all(dir.join("hello/files")).expect("removed");
        fs::write(&record, format!("digest {digest}\n")).expect("written");
        assert_eq!(store.load("hello").0, None, "missing files");
        fs::remove_dir_all(dir).ok();
    }
}

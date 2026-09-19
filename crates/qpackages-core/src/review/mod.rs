//! Reviewing an AUR recipe before it is built.
//!
//! Building an AUR package runs a script its maintainer wrote, and installing it runs another as
//! root. A package the user trusted months ago can change hands. So before every install and
//! update the recipe is compared with the one the user last approved, and its lines are scanned
//! for patterns worth a look:
//!
//! - the recipe is the same, or only its version and checksums moved: nothing to read;
//! - anything else changed: the changed lines are shown;
//! - the [rules](rules::Rule) point at lines that fetch, hide or elevate, and at a new source host
//!   or a new maintainer.
//!
//! The approval is kept by [`Store`]: a digest of the recipe, its maintainer and source hosts,
//! and a copy of its files to compare the next version with.

pub mod diff;
pub mod rules;
mod sha256;
mod store;

pub use rules::{Finding, Rule};
pub use store::{Approved, Diagnostic, Store};

use diff::{Kind, Line, diff_lines};
use rules::{arrays, host_of, url_of};

/// One file of a recipe: the `PKGBUILD`, an install script, or a local file its sources name.
///
/// `.SRCINFO` does not belong here: it is generated from the `PKGBUILD` and changes whenever
/// the version does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeFile {
    /// The file name, without a folder.
    pub name: String,
    /// Its text.
    pub text: String,
}

/// A recipe: its files, in any order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recipe {
    /// The files.
    pub files: Vec<RecipeFile>,
}

impl Recipe {
    /// The SHA-256 digest of the recipe, the same whatever order its files come in.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut files: Vec<&RecipeFile> = self.files.iter().collect();
        files.sort_by(|left, right| left.name.cmp(&right.name));
        // Each file is framed by its name and length, so moving text from one file to another
        // changes the digest.
        let mut bytes = Vec::new();
        for file in files {
            bytes.extend(file.name.as_bytes());
            bytes.push(0);
            bytes.extend(file.text.len().to_string().as_bytes());
            bytes.push(0);
            bytes.extend(file.text.as_bytes());
        }
        sha256::hex_digest(&bytes)
    }

    /// The hosts the `PKGBUILD`'s sources download from, sorted and once each.
    #[must_use]
    pub fn domains(&self) -> Vec<String> {
        let Some(pkgbuild) = self.files.iter().find(|file| file.name == "PKGBUILD") else {
            return Vec::new();
        };
        let mut hosts: Vec<String> = arrays(&pkgbuild.text, |name| name == "source" || name.starts_with("source_"))
            .iter()
            .filter_map(|value| url_of(&value.text).and_then(host_of))
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    fn file(&self, name: &str) -> Option<&RecipeFile> {
        self.files.iter().find(|file| file.name == name)
    }
}

/// How a recipe compares with the one last approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// No version of this package was approved before: the whole recipe is to be read.
    New,
    /// The recipe is exactly the one approved.
    Same,
    /// Only the version, the release and the checksums changed: the package can update without
    /// being read again.
    OnlyVersion,
    /// Something else changed; the files that did, with their lines.
    Changed(Vec<FileDiff>),
}

/// The lines of one file that changed, among the ones that did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// The file name.
    pub name: String,
    /// Every line of the two versions, marked.
    pub lines: Vec<Line>,
}

/// What a review found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// How the recipe compares with the approved one.
    pub change: Change,
    /// What the rules point at, in file order.
    pub findings: Vec<Finding>,
}

impl Review {
    /// Whether the package may update without the user reading it: its recipe is the approved
    /// one or differs only in its version, and no rule points at anything.
    #[must_use]
    pub fn needs_no_reading(&self) -> bool {
        matches!(self.change, Change::Same | Change::OnlyVersion) && self.findings.is_empty()
    }
}

/// Reviews `recipe`, whose package the AUR says `maintainer` keeps (`None` when orphaned),
/// against the version the user last approved, if any.
#[must_use]
pub fn review(recipe: &Recipe, maintainer: Option<&str>, approved: Option<&Approved>) -> Review {
    let mut findings = rules::scan(&recipe.files);
    let change = approved.map_or(Change::New, |approved| compare(&approved.recipe, recipe));
    if let Some(approved) = approved {
        let pkgbuild = recipe.file("PKGBUILD");
        for domain in recipe.domains() {
            if !approved.domains.contains(&domain) {
                let line = pkgbuild.and_then(|file| {
                    file.text.lines().position(|line| line.to_ascii_lowercase().contains(&domain)).map(|at| at + 1)
                });
                findings.push(Finding { rule: Rule::NewDomain, file: "PKGBUILD".to_owned(), line, detail: domain });
            }
        }
        if approved.maintainer.as_deref() != maintainer {
            findings.push(Finding {
                rule: Rule::NewMaintainer,
                file: "PKGBUILD".to_owned(),
                line: None,
                detail: maintainer.unwrap_or_default().to_owned(),
            });
        }
    }
    Review { change, findings }
}

/// How `new` compares with `old`.
fn compare(old: &Recipe, new: &Recipe) -> Change {
    if old.digest() == new.digest() {
        return Change::Same;
    }
    let mut names: Vec<&str> = old.files.iter().chain(&new.files).map(|file| file.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let only_version = names.iter().all(|name| {
        let (before, after) = (text(old, name), text(new, name));
        if *name == "PKGBUILD" { without_version(before) == without_version(after) } else { before == after }
    }) && names.iter().all(|name| old.file(name).is_some() == new.file(name).is_some());
    if only_version {
        return Change::OnlyVersion;
    }
    let diffs = names
        .into_iter()
        .map(|name| FileDiff { name: name.to_owned(), lines: diff_lines(text(old, name), text(new, name)) })
        .filter(|file| file.lines.iter().any(|line| line.kind != Kind::Same))
        .collect();
    Change::Changed(diffs)
}

/// The text of file `name` of `recipe`, empty when it has no such file.
fn text<'a>(recipe: &'a Recipe, name: &str) -> &'a str {
    recipe.file(name).map_or("", |file| file.text.as_str())
}

/// The `PKGBUILD`'s lines without the ones that move with every release: `pkgver`, `pkgrel`,
/// `epoch` and the checksum arrays, however many lines those take.
fn without_version(pkgbuild: &str) -> Vec<&str> {
    let mut kept = Vec::new();
    let mut in_sums = false;
    for line in pkgbuild.lines() {
        let trimmed = line.trim_start();
        if in_sums {
            in_sums = !line.contains(')');
            continue;
        }
        let assigned = trimmed.split_once('=').map(|(name, _)| name);
        let is_sums = assigned.is_some_and(|name| {
            let base = name.split_once('_').map_or(name, |(base, _)| base);
            base.ends_with("sums") && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
        if is_sums {
            in_sums = trimmed.contains("=(") && !trimmed.contains(')');
            continue;
        }
        if !matches!(assigned, Some("pkgver" | "pkgrel" | "epoch")) {
            kept.push(line);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = "pkgname=hello\npkgver=2.12\npkgrel=1\nsource=('https://ftp.gnu.org/hello-$pkgver.tar.gz')\n\
        sha256sums=('aaaa'\n            'bbbb')\nbuild() {\n  make\n}\n";

    fn recipe(pkgbuild: &str) -> Recipe {
        Recipe { files: vec![RecipeFile { name: "PKGBUILD".to_owned(), text: pkgbuild.to_owned() }] }
    }

    fn approved(pkgbuild: &str, maintainer: Option<&str>) -> Approved {
        let recipe = recipe(pkgbuild);
        Approved {
            digest: recipe.digest(),
            maintainer: maintainer.map(str::to_owned),
            domains: recipe.domains(),
            recipe,
        }
    }

    #[test]
    fn the_digest_ignores_file_order_and_frames_each_file() {
        let a = RecipeFile { name: "PKGBUILD".to_owned(), text: "x".to_owned() };
        let b = RecipeFile { name: "a.install".to_owned(), text: "y".to_owned() };
        let one = Recipe { files: vec![a.clone(), b.clone()] };
        let two = Recipe { files: vec![b, a] };
        assert_eq!(one.digest(), two.digest());
        let moved = Recipe {
            files: vec![
                RecipeFile { name: "PKGBUILD".to_owned(), text: "xy".to_owned() },
                RecipeFile { name: "a.install".to_owned(), text: String::new() },
            ],
        };
        assert_ne!(one.digest(), moved.digest());
    }

    #[test]
    fn a_first_review_reads_the_whole_recipe() {
        let review = review(&recipe(V1), Some("someone"), None);
        assert_eq!(review.change, Change::New);
        assert!(review.findings.is_empty());
        assert!(!review.needs_no_reading());
    }

    #[test]
    fn the_same_recipe_needs_no_reading() {
        let review = review(&recipe(V1), Some("someone"), Some(&approved(V1, Some("someone"))));
        assert_eq!(review.change, Change::Same);
        assert!(review.needs_no_reading());
    }

    #[test]
    fn a_new_version_with_new_checksums_needs_no_reading() {
        let v2 = V1.replace("pkgver=2.12", "pkgver=2.13").replace("pkgrel=1", "pkgrel=2").replace("'aaaa'", "'cccc'");
        let v2 = v2.replace("            'bbbb')", "            'dddd'\n            'eeee')");
        let review = review(&recipe(&v2), Some("someone"), Some(&approved(V1, Some("someone"))));
        assert_eq!(review.change, Change::OnlyVersion);
        assert!(review.needs_no_reading());
    }

    #[test]
    fn any_other_change_shows_its_lines() {
        let v2 = V1.replace("  make\n", "  make\n  make check\n");
        let review = review(&recipe(&v2), Some("someone"), Some(&approved(V1, Some("someone"))));
        let Change::Changed(files) = &review.change else { panic!("a changed build is a change: {:?}", review.change) };
        assert_eq!(files.len(), 1);
        let added: Vec<(&str, Option<usize>)> = files[0]
            .lines
            .iter()
            .filter(|line| line.kind == Kind::Added)
            .map(|line| (line.text.as_str(), line.new))
            .collect();
        assert_eq!(added, [("  make check", Some(9))]);
        assert!(!review.needs_no_reading());
    }

    #[test]
    fn a_new_file_is_a_change_even_when_the_pkgbuild_only_moved_its_version() {
        let mut new = recipe(&V1.replace("pkgver=2.12", "pkgver=2.13"));
        new.files
            .push(RecipeFile { name: "hello.install".to_owned(), text: "post_install() {\n  true\n}\n".to_owned() });
        let review = review(&new, Some("someone"), Some(&approved(V1, Some("someone"))));
        let Change::Changed(files) = review.change else { panic!("a new install script is a change") };
        assert_eq!(
            files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            ["PKGBUILD", "hello.install"],
            "once something is to be read, the version line is shown with the rest"
        );
    }

    #[test]
    fn a_new_source_host_and_a_new_maintainer_are_findings() {
        let moved = V1.replace("https://ftp.gnu.org/", "https://mirror.example.net/");
        let review = review(&recipe(&moved), Some("newcomer"), Some(&approved(V1, Some("someone"))));
        let found: Vec<(Rule, Option<usize>, &str)> =
            review.findings.iter().map(|finding| (finding.rule, finding.line, finding.detail.as_str())).collect();
        assert_eq!(found, [(Rule::NewDomain, Some(4), "mirror.example.net"), (Rule::NewMaintainer, None, "newcomer")]);
        let orphaned = review_of_orphaned();
        assert!(orphaned.findings.iter().any(|finding| finding.rule == Rule::NewMaintainer), "orphaned is a change");
    }

    fn review_of_orphaned() -> Review {
        review(&recipe(V1), None, Some(&approved(V1, Some("someone"))))
    }

    #[test]
    fn the_same_host_and_maintainer_are_not_findings() {
        let review = review(&recipe(V1), Some("someone"), Some(&approved(V1, Some("someone"))));
        assert!(review.findings.is_empty());
        assert_eq!(recipe(V1).domains(), ["ftp.gnu.org"]);
    }
}

//! Reading every installed package out of pacman's local database directory.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Package, parse_desc};

/// Something that could not be read, naming the file it came from.
///
/// A broken record is skipped rather than failing the whole read: a package manager that
/// refuses to list anything because one record is damaged is worse than one that lists the
/// rest and says what it could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The file the trouble came from.
    pub file: PathBuf,
    /// What went wrong, in English, for a diagnostic rather than for the user.
    pub reason: String,
}

/// Reads every installed package from a pacman local database directory, usually
/// `/var/lib/pacman/local`.
///
/// The directory holds one folder per package, each with a `desc` file. Packages come back in
/// name order. A record that cannot be read becomes a [`Problem`] and the rest are still
/// returned, so one damaged record never hides the other fifteen hundred.
#[must_use]
pub fn read_local_db(dir: &Path) -> (Vec<Package>, Vec<Problem>) {
    let mut packages = Vec::new();
    let mut problems = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(Problem { file: dir.to_path_buf(), reason: error.to_string() });
            return (packages, problems);
        }
    };
    for entry in entries {
        let record = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                problems.push(Problem { file: dir.to_path_buf(), reason: error.to_string() });
                continue;
            }
        };
        if !record.is_dir() {
            continue;
        }
        let file = record.join("desc");
        match fs::read_to_string(&file) {
            Ok(text) => match parse_desc(&text) {
                Some(package) => packages.push(package),
                None => problems.push(Problem { file, reason: "the record names no package".to_owned() }),
            },
            Err(error) => problems.push(Problem { file, reason: error.to_string() }),
        }
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    (packages, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/localdb")
    }

    #[test]
    fn reads_the_packages_and_reports_what_it_could_not_read() {
        let (packages, problems) = read_local_db(&fixture());
        assert_eq!(packages.len(), 1, "only one of the three records names a package");
        assert_eq!(packages[0].name, "bash");
        assert_eq!(packages[0].size, Some(10_055_508));
        assert_eq!(problems.len(), 2, "the nameless record and the one without a desc file");
        assert!(
            problems.iter().any(|problem| problem.file.ends_with("nameless-1.0-1/desc")),
            "a record without a name is reported, not silently dropped"
        );
        assert!(problems.iter().any(|problem| problem.file.ends_with("broken-2.0-1/desc")));
    }

    #[test]
    fn packages_come_back_in_name_order() {
        let (packages, _) = read_local_db(&fixture());
        let mut sorted = packages.clone();
        sorted.sort_by(|left, right| left.name.cmp(&right.name));
        assert_eq!(packages, sorted, "the caller should not have to sort before showing");
    }

    /// Reads the database of the machine this runs on. Ignored by default because it depends on
    /// that machine; run it with `cargo test -- --ignored` after changing the reader.
    #[test]
    #[ignore = "depends on the machine's own package database"]
    fn reads_the_database_of_this_machine() {
        let (packages, problems) = read_local_db(Path::new("/var/lib/pacman/local"));
        assert!(packages.len() > 100, "a working Arch system has more packages than that");
        assert_eq!(problems, [], "every record of a healthy database reads cleanly");
        assert!(packages.iter().any(|package| package.name == "pacman"), "pacman installed itself");
        assert!(packages.iter().all(|package| !package.version.is_empty()), "every record carries a version");
    }

    #[test]
    fn a_directory_that_is_not_there_is_one_problem_and_no_packages() {
        let (packages, problems) = read_local_db(&fixture().join("does-not-exist"));
        assert!(packages.is_empty());
        assert_eq!(problems.len(), 1, "the missing directory is reported once");
    }
}

//! Which files a package installed, read from the `files` record pacman keeps beside `desc`.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The folder of one package's records in the local database: `<db>/<name>-<version>`.
#[must_use]
pub fn record_dir(db: &Path, name: &str, version: &str) -> PathBuf {
    db.join(format!("{name}-{version}"))
}

/// The paths under `prefix` the package whose records are in `record` installed, as pacman writes
/// them: relative to `/`, such as `usr/share/applications/firefox.desktop`.
///
/// The `files` record lists every path of the package after a `%FILES%` line, up to a blank line;
/// the record is read line by line, so a package with tens of thousands of files is never held in
/// memory whole. A missing or unreadable record is an error; lines that are not valid text are
/// skipped rather than failing the rest.
pub fn files_under(record: &Path, prefix: &str) -> io::Result<Vec<String>> {
    let reader = BufReader::new(File::open(record.join("files"))?);
    let mut in_files = false;
    let mut found = Vec::new();
    for line in reader.split(b'\n') {
        let line = line?;
        let Ok(line) = std::str::from_utf8(&line) else {
            continue;
        };
        if line.is_empty() {
            in_files = false;
        } else if line.starts_with('%') && line.ends_with('%') {
            in_files = line == "%FILES%";
        } else if in_files && line.starts_with(prefix) {
            found.push(line.to_owned());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory can be created");
        dir
    }

    #[test]
    fn only_the_files_under_the_prefix_come_back() {
        let dir = scratch("prefix");
        let record = record_dir(&dir, "firefox", "143.0-1");
        fs::create_dir_all(&record).expect("the record folder");
        let text = "%FILES%\nusr/\nusr/bin/firefox\nusr/share/applications/\n\
                    usr/share/applications/firefox.desktop\nusr/share/icons/x.png\n\n\
                    %BACKUP%\nusr/share/applications/not-a-file-entry\n";
        fs::write(record.join("files"), text).expect("the record");
        let found = files_under(&record, "usr/share/applications/").expect("readable");
        assert_eq!(found, ["usr/share/applications/", "usr/share/applications/firefox.desktop"]);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_missing_record_is_an_error_and_broken_text_is_skipped() {
        let dir = scratch("missing");
        assert!(files_under(&dir.join("nothing-1-1"), "usr/").is_err());
        let record = record_dir(&dir, "odd", "1-1");
        fs::create_dir_all(&record).expect("the record folder");
        fs::write(record.join("files"), b"%FILES%\nusr/\xff\xfe\nusr/share/a.desktop\n").expect("the record");
        assert_eq!(files_under(&record, "usr/share/").expect("readable"), ["usr/share/a.desktop"]);
        fs::remove_dir_all(dir).expect("cleanup");
    }
}

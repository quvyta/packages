//! Which installed packages are applications: the ones that put a launcher in the desktop's
//! application menu, the way GNOME Software and KDE Discover decide what to call an app.
//!
//! A launcher is a `.desktop` file in the applications folder, normally
//! `/usr/share/applications`, that says it is an application and does not hide itself. The
//! package it belongs to is read from pacman's own record of each package's files. Everything
//! here only reads; a file that cannot be read is skipped.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use qpackages_core::pacman::Package;
use qpackages_core::pacman::files::{files_under, record_dir};

/// Where launchers live, relative to `/`, as pacman's file lists write paths.
const LAUNCHERS: &str = "usr/share/applications/";

/// The names of the launchers in `folder` that show in an application menu.
#[must_use]
pub fn launchers(folder: &Path) -> BTreeSet<String> {
    let Ok(entries) = fs::read_dir(folder) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let shown = name.ends_with(".desktop") && fs::read_to_string(entry.path()).is_ok_and(|text| shows(&text));
            shown.then_some(name)
        })
        .collect()
}

/// Whether a launcher's text makes it an application in the menu: in its `[Desktop Entry]`
/// group, `Type=Application` and neither `NoDisplay=true` nor `Hidden=true`. Other groups, such
/// as a launcher's extra actions, say nothing about the launcher itself.
fn shows(text: &str) -> bool {
    let mut in_entry = false;
    let mut application = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        match line.split_once('=').map(|(key, value)| (key.trim(), value.trim())) {
            Some(("Type", value)) => application = value == "Application",
            Some(("NoDisplay" | "Hidden", "true")) => return false,
            _ => {}
        }
    }
    application
}

/// The names of the `packages` recorded in the local database at `db` that installed one of
/// `launchers`.
#[must_use]
pub fn owners(db: &Path, packages: &[Package], launchers: &BTreeSet<String>) -> BTreeSet<String> {
    if launchers.is_empty() {
        return BTreeSet::new();
    }
    packages
        .iter()
        .filter(|package| {
            let record = record_dir(db, &package.name, &package.version);
            files_under(&record, LAUNCHERS).is_ok_and(|files| {
                files.iter().any(|file| file.strip_prefix(LAUNCHERS).is_some_and(|name| launchers.contains(name)))
            })
        })
        .map(|package| package.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qpackages-apps-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory can be created");
        dir
    }

    #[test]
    fn only_shown_applications_are_launchers() {
        assert!(shows("[Desktop Entry]\nType=Application\nName=Firefox\n"));
        assert!(!shows("[Desktop Entry]\nType=Application\nNoDisplay=true\n"));
        assert!(!shows("[Desktop Entry]\nType=Application\nHidden=true\n"));
        assert!(!shows("[Desktop Entry]\nType=Link\n"));
        assert!(!shows("Type=Application\n"), "a key outside the group says nothing");
        assert!(
            shows("[Desktop Entry]\nType = Application\n\n[Desktop Action new]\nNoDisplay=true\n"),
            "an action's keys belong to the action"
        );
    }

    #[test]
    fn the_owners_are_the_packages_whose_file_list_holds_a_launcher() {
        let dir = scratch("owners");
        let folder = dir.join("applications");
        fs::create_dir_all(&folder).expect("the launcher folder");
        fs::write(folder.join("firefox.desktop"), "[Desktop Entry]\nType=Application\n").expect("a launcher");
        fs::write(folder.join("helper.desktop"), "[Desktop Entry]\nType=Application\nNoDisplay=true\n")
            .expect("a hidden launcher");
        fs::write(folder.join("notes.txt"), "[Desktop Entry]\nType=Application\n").expect("not a launcher");
        let db = dir.join("local");
        let packages: Vec<Package> = [
            ("firefox", "usr/share/applications/firefox.desktop"),
            ("helper", "usr/share/applications/helper.desktop"),
            ("bash", "usr/bin/bash"),
        ]
        .iter()
        .map(|(name, file)| {
            let record = record_dir(&db, name, "1-1");
            fs::create_dir_all(&record).expect("a record");
            fs::write(record.join("files"), format!("%FILES%\nusr/\n{file}\n")).expect("a file list");
            Package { name: (*name).to_owned(), version: "1-1".to_owned(), ..Package::default() }
        })
        .chain([Package { name: "listless".to_owned(), version: "1-1".to_owned(), ..Package::default() }])
        .collect();

        let found = launchers(&folder);
        assert_eq!(found, BTreeSet::from(["firefox.desktop".to_owned()]));
        assert_eq!(owners(&db, &packages, &found), BTreeSet::from(["firefox".to_owned()]));
        assert!(launchers(&dir.join("nowhere")).is_empty(), "a missing folder has no launchers");
        fs::remove_dir_all(dir).expect("cleanup");
    }
}

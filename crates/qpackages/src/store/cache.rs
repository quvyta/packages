//! The rankings the network gave, kept on disk: pkgstats' list, Flathub's two lists (most
//! installed, latest updated) and what the AUR said about the AUR row.
//!
//! Each answer is kept as the network sent it, one file per ranking, and read back with the same
//! parser. On start the page shows what is kept, so the rows take the ranking's order at once and
//! do not wait for the network; an answer younger than a day is not asked for again. Everything
//! here is best effort: a file that cannot be read or written is as if there were no cache, and
//! the network answers as it would without one.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// How long a kept answer counts as current. The rankings move slowly; asking once a day is
/// plenty and spares the servers.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// A ranking kept on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// pkgstats' most used packages.
    Pkgstats,
    /// Flathub's most installed applications.
    FlathubPopular,
    /// What Flathub updated last.
    FlathubRecent,
    /// What the AUR said about the packages of the AUR row.
    AurRow,
}

impl Kept {
    /// The file it is kept in, inside the cache folder.
    pub const fn file(self) -> &'static str {
        match self {
            Self::Pkgstats => "pkgstats.json",
            Self::FlathubPopular => "flathub-popular.json",
            Self::FlathubRecent => "flathub-recent.json",
            Self::AurRow => "aur-row.json",
        }
    }
}

/// A kept answer and whether it is recent enough not to ask again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// The answer as the network sent it.
    pub text: String,
    /// Whether it is younger than [`MAX_AGE`] at the time it was read.
    pub fresh: bool,
}

/// The answer kept for `kept` in `folder`, read at `now`; `None` when there is none or it cannot
/// be read. A file dated in the future (a clock set back) counts as old, so it is asked again.
pub fn read(folder: &Path, kept: Kept, now: SystemTime) -> Option<Body> {
    let path = folder.join(kept.file());
    let text = std::fs::read_to_string(&path).ok()?;
    let modified = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok();
    let fresh = modified.and_then(|modified| now.duration_since(modified).ok()).is_some_and(|age| age < MAX_AGE);
    Some(Body { text, fresh })
}

/// Keeps `text` as the answer for `kept` in `folder`, creating the folder. It is written beside
/// the old file and moved over it, so a reader never sees half an answer. A failure leaves the
/// old answer, or none; the next start asks the network again.
pub fn write(folder: &Path, kept: Kept, text: &str) {
    let path = folder.join(kept.file());
    let partial = folder.join(format!(".{}.partial", kept.file()));
    let written = std::fs::create_dir_all(folder)
        .and_then(|()| std::fs::write(&partial, text))
        .and_then(|()| std::fs::rename(&partial, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::path::PathBuf;

    use super::*;

    /// A folder of its own under the system's temporary place, gone when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("qpackages-store-cache-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_written_answer_reads_back_fresh_and_turns_old_after_a_day() {
        let scratch = Scratch::new("age");
        let folder = scratch.0.join("quvyta/packages");
        assert_eq!(read(&folder, Kept::Pkgstats, SystemTime::now()), None, "nothing kept yet");
        write(&folder, Kept::Pkgstats, "{\"packagePopularities\":[]}");
        let now = SystemTime::now();
        let body = read(&folder, Kept::Pkgstats, now).expect("kept");
        assert_eq!(body, Body { text: String::from("{\"packagePopularities\":[]}"), fresh: true });
        let later = now + MAX_AGE + Duration::from_secs(1);
        assert!(!read(&folder, Kept::Pkgstats, later).expect("still kept").fresh);
        let earlier = now - Duration::from_secs(3600);
        assert!(!read(&folder, Kept::Pkgstats, earlier).expect("kept").fresh, "a file from the future is old");
        assert_eq!(read(&folder, Kept::AurRow, now), None, "each ranking has its own file");
    }

    #[test]
    fn a_new_answer_replaces_the_old_one_and_its_age() {
        let scratch = Scratch::new("replace");
        write(&scratch.0, Kept::FlathubPopular, "old");
        let file = File::options().write(true).open(scratch.0.join(Kept::FlathubPopular.file())).expect("the file");
        file.set_modified(SystemTime::now() - MAX_AGE * 2).expect("the date is set");
        assert!(!read(&scratch.0, Kept::FlathubPopular, SystemTime::now()).expect("kept").fresh);
        write(&scratch.0, Kept::FlathubPopular, "new");
        let body = read(&scratch.0, Kept::FlathubPopular, SystemTime::now()).expect("kept");
        assert_eq!(body, Body { text: String::from("new"), fresh: true });
        let names: Vec<_> = std::fs::read_dir(&scratch.0).expect("the folder").filter_map(Result::ok).collect();
        assert_eq!(names.len(), 1, "no partial file is left behind");
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_no_error() {
        let scratch = Scratch::new("blocked");
        std::fs::write(&scratch.0, "a file where the folder should be").expect("a file");
        write(&scratch.0, Kept::Pkgstats, "text");
        assert_eq!(read(&scratch.0, Kept::Pkgstats, SystemTime::now()), None);
        let _ = std::fs::remove_file(&scratch.0);
    }
}

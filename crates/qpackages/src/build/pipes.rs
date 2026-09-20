//! The private pair of pipes between one AUR build's shims and the qpac that runs the build.
//!
//! Each build gets a folder of its own with a random name under the user's runtime folder,
//! `quvyta-packages/<random>/`, readable by the user alone, holding two named pipes: the shim
//! writes its request to `request` and reads the answer from `answer`. The folder goes when the
//! build ends.

use std::fs::{self, DirBuilder, File};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, FileType, Mode, mknodat};

/// The pipe the shim writes its request to.
pub const REQUEST: &str = "request";

/// The pipe the shim reads its answer from.
pub const ANSWER: &str = "answer";

/// The folder under the runtime folder that holds every build's own.
const FOLDER: &str = "quvyta-packages";

/// Only the owner may enter the folders or use the pipes.
const FOLDER_MODE: u32 = 0o700;
const PIPE_MODE: u32 = 0o600;

/// One build's folder and its two pipes, removed when dropped.
#[derive(Debug)]
pub struct Pipes {
    folder: PathBuf,
}

impl Pipes {
    /// Makes a new folder with a random name under `base`'s [`FOLDER`] for user `uid`, and the two
    /// pipes in it.
    ///
    /// # Errors
    ///
    /// Returns why the folder or a pipe could not be made, or the shared folder is not the
    /// user's own; nothing is left behind then.
    pub fn create(base: &Path, uid: u32) -> io::Result<Self> {
        let shared = base.join(FOLDER);
        match DirBuilder::new().mode(FOLDER_MODE).create(&shared) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        // A folder someone else made, or a link put in its place, is never used.
        let seen = fs::symlink_metadata(&shared)?;
        if !seen.is_dir() || seen.uid() != uid {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        fs::set_permissions(&shared, fs::Permissions::from_mode(FOLDER_MODE))?;
        let folder = shared.join(random_name()?);
        DirBuilder::new().mode(FOLDER_MODE).create(&folder)?;
        let pipes = Self { folder };
        fs::set_permissions(&pipes.folder, fs::Permissions::from_mode(FOLDER_MODE))?;
        for pipe in [pipes.request(), pipes.answer()] {
            mknodat(CWD, &pipe, FileType::Fifo, Mode::from_raw_mode(PIPE_MODE), 0)?;
            fs::set_permissions(&pipe, fs::Permissions::from_mode(PIPE_MODE))?;
        }
        Ok(pipes)
    }

    /// The folder, which the shim is given.
    #[must_use]
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The request pipe.
    #[must_use]
    pub fn request(&self) -> PathBuf {
        self.folder.join(REQUEST)
    }

    /// The answer pipe.
    #[must_use]
    pub fn answer(&self) -> PathBuf {
        self.folder.join(ANSWER)
    }
}

impl Drop for Pipes {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.request());
        let _ = fs::remove_file(self.answer());
        let _ = fs::remove_dir(&self.folder);
    }
}

/// Whether `folder` is a build's folder that user `uid` made: a folder of theirs nobody else may
/// enter, with the two pipes of theirs in it. The shim checks this before it writes anything.
///
/// # Errors
///
/// Returns [`io::ErrorKind::PermissionDenied`] when it is not, or the error of a look that failed.
pub fn check(folder: &Path, uid: u32) -> io::Result<()> {
    let refused = || io::Error::from(io::ErrorKind::PermissionDenied);
    if !folder.is_absolute() {
        return Err(refused());
    }
    let seen = fs::symlink_metadata(folder)?;
    if !seen.is_dir() || seen.uid() != uid || seen.mode() & 0o077 != 0 {
        return Err(refused());
    }
    for pipe in [REQUEST, ANSWER] {
        let seen = fs::symlink_metadata(folder.join(pipe))?;
        if !seen.file_type().is_fifo() || seen.uid() != uid {
            return Err(refused());
        }
    }
    Ok(())
}

/// Sixteen random bytes as hexadecimal, from the kernel's generator.
fn random_name() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh folder standing in for the runtime folder.
    pub(crate) fn base(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("qpackages-pipes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a scratch folder");
        base
    }

    fn uid() -> u32 {
        crate::current_uid().expect("this process's user")
    }

    #[test]
    fn a_build_gets_a_private_folder_with_two_pipes_that_goes_with_it() {
        let base = base("create");
        let pipes = Pipes::create(&base, uid()).expect("the pipes are made");
        let folder = pipes.folder().to_path_buf();
        assert_eq!(folder.parent(), Some(base.join(FOLDER).as_path()));
        assert_eq!(folder.file_name().map(std::ffi::OsStr::len), Some(32));
        assert_eq!(fs::metadata(&folder).expect("the folder").mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(base.join(FOLDER)).expect("the shared folder").mode() & 0o777, 0o700);
        for pipe in [pipes.request(), pipes.answer()] {
            let seen = fs::metadata(&pipe).expect("a pipe");
            assert!(seen.file_type().is_fifo(), "{}", pipe.display());
            assert_eq!(seen.mode() & 0o777, 0o600);
        }
        assert_eq!(check(&folder, uid()).map_err(|error| error.kind()), Ok(()));
        let other = Pipes::create(&base, uid()).expect("a second build");
        assert_ne!(other.folder(), folder, "every build its own folder");
        drop(pipes);
        assert!(!folder.exists(), "the folder goes with the build");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn a_folder_that_is_not_a_private_build_folder_is_refused() {
        let base = base("check");
        let denied = |folder: &Path, uid| check(folder, uid).map_err(|error| error.kind());
        let pipes = Pipes::create(&base, uid()).expect("the pipes are made");
        assert_eq!(denied(pipes.folder(), uid() + 1), Err(io::ErrorKind::PermissionDenied), "someone else's");
        assert_eq!(denied(Path::new("relative"), uid()), Err(io::ErrorKind::PermissionDenied));
        fs::set_permissions(pipes.folder(), fs::Permissions::from_mode(0o750)).expect("chmod");
        assert_eq!(denied(pipes.folder(), uid()), Err(io::ErrorKind::PermissionDenied), "others may enter");
        fs::set_permissions(pipes.folder(), fs::Permissions::from_mode(0o700)).expect("chmod");
        fs::remove_file(pipes.answer()).expect("remove the pipe");
        fs::write(pipes.answer(), "").expect("a plain file in its place");
        assert_eq!(denied(pipes.folder(), uid()), Err(io::ErrorKind::PermissionDenied), "not a pipe");
        let link = base.join("link");
        std::os::unix::fs::symlink(pipes.folder(), &link).expect("a link");
        assert_eq!(denied(&link, uid()), Err(io::ErrorKind::PermissionDenied), "a link to the folder");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn a_shared_folder_that_is_a_link_is_not_used() {
        let base = base("link");
        let elsewhere = base.join("elsewhere");
        fs::create_dir(&elsewhere).expect("a folder");
        std::os::unix::fs::symlink(&elsewhere, base.join(FOLDER)).expect("a link");
        let made = Pipes::create(&base, uid()).map(|_| ()).map_err(|error| error.kind());
        assert_eq!(made, Err(io::ErrorKind::PermissionDenied));
        assert_eq!(fs::read_dir(&elsewhere).expect("readable").count(), 0, "nothing was made through the link");
        let _ = fs::remove_dir_all(base);
    }
}

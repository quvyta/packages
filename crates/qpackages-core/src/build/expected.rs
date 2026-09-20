//! The second check on a shim's request: only what the build in progress is expected to ask for.
//!
//! The shim's pipes sit in a private folder, but any program of the same user could find them and
//! write a request. So the qpac that started the build passes a request on only when it belongs
//! to that build: package files named after the packages it builds, dependencies it listed, and
//! its own targets marked as installed on purpose. The helper checks the files once more where it
//! opens them.

use std::path::{Path, PathBuf};

use super::plan::Plan;
use crate::helper::{BUILT_ENDINGS, Refusal, Request, in_build_cache};

/// What one AUR build may ask the helper for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    /// The home folder of the user building, whose build cache the files must come from.
    home: PathBuf,
    /// The packages the user asked for.
    targets: Vec<String>,
    /// Every package the build makes, the targets and the AUR dependencies.
    built: Vec<String>,
    /// Every dependency the build may install, mark or remove again: those from the repositories
    /// and those it builds from the AUR.
    dependencies: Vec<String>,
    /// The debug packages of what the build makes, which yay installs beside them and marks as
    /// dependencies.
    debug: Vec<String>,
}

impl Expected {
    /// What building `plan` as the user whose home folder is `home` may ask for.
    #[must_use]
    pub fn new(home: &Path, plan: &Plan) -> Self {
        let built: Vec<String> = plan.builds.iter().map(|build| build.name.clone()).collect();
        let mut dependencies: Vec<String> = plan.repo.steps.iter().map(|step| step.name.clone()).collect();
        dependencies.extend(built.iter().filter(|name| !plan.targets.contains(name)).cloned());
        let debug = built.iter().map(|name| format!("{name}-debug")).collect();
        Self { home: home.to_path_buf(), targets: plan.targets.clone(), built, dependencies, debug }
    }

    /// Whether `request` belongs to this build.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::NotThisBuild`] for anything the build was not expected to ask for.
    pub fn check(&self, request: &Request) -> Result<(), Refusal> {
        let within = |names: &[String], allowed: &[String]| names.iter().all(|name| allowed.contains(name));
        let belongs = match request {
            Request::InstallBuilt(paths) => paths.iter().all(|path| self.is_built_here(path)),
            Request::Install(names) | Request::Remove(names) => within(names, &self.dependencies),
            Request::MarkDeps(names) => {
                names.iter().all(|name| self.dependencies.contains(name) || self.debug.contains(name))
            }
            Request::MarkExplicit(names) => within(names, &self.targets),
            _ => false,
        };
        if belongs { Ok(()) } else { Err(Refusal::NotThisBuild) }
    }

    /// Whether `path` is in the user's build cache and names a package this build makes, as
    /// makepkg names its files: `<name>-<version>-<release>-<architecture>` and an ending, with
    /// `-debug` after the name for the debug package beside it.
    fn is_built_here(&self, path: &str) -> bool {
        let path = Path::new(path);
        let Some(file) = path.file_name().and_then(|name| name.to_str()) else { return false };
        let Some(stem) = BUILT_ENDINGS.iter().find_map(|ending| file.strip_suffix(ending)) else { return false };
        let names_this = |name: &String| {
            stem.strip_prefix(name.as_str()).and_then(|rest| rest.strip_prefix('-')).is_some_and(|rest| {
                let rest = rest.strip_prefix("debug-").filter(|shorter| is_version(shorter)).unwrap_or(rest);
                is_version(rest)
            })
        };
        in_build_cache(path, &self.home) && self.built.iter().any(names_this)
    }
}

/// Whether `rest` is `<version>-<release>-<architecture>`: three parts, none empty, none with a
/// dash, since makepkg allows none in any of them.
fn is_version(rest: &str) -> bool {
    let parts: Vec<&str> = rest.split('-').collect();
    parts.len() == 3 && parts.iter().all(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::plan::Built;
    use crate::pacman::{Plan as RepoPlan, Step};

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// `hello` from the AUR, which needs `libgreet` from the AUR and `scdoc` from the
    /// repositories to build.
    fn expected() -> Expected {
        let build = |name: &str| Built {
            name: name.to_owned(),
            base: name.to_owned(),
            version: "1.0-1".to_owned(),
            maintainer: Some("someone".to_owned()),
        };
        let plan = Plan {
            targets: owned(&["hello"]),
            builds: vec![build("libgreet"), build("hello")],
            repo: RepoPlan {
                steps: vec![Step {
                    repo: Some("extra".to_owned()),
                    name: "scdoc".to_owned(),
                    version: "1.11.3-1".to_owned(),
                    size: None,
                }],
            },
        };
        Expected::new(Path::new("/home/builder"), &plan)
    }

    fn built(paths: &[&str]) -> Request {
        Request::InstallBuilt(owned(paths))
    }

    #[test]
    fn the_builds_own_files_go_through() {
        let expected = expected();
        for path in [
            "/home/builder/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst",
            "/home/builder/.cache/paru/clone/hello/hello-debug-1.0-1-x86_64.pkg.tar.zst",
            "/home/builder/.cache/yay/libgreet/libgreet-2:0.3.r4.g1bf1379-2-any.pkg.tar.xz",
        ] {
            assert_eq!(expected.check(&built(&[path])), Ok(()), "{path}");
        }
    }

    #[test]
    fn a_file_of_another_package_or_place_is_refused() {
        let expected = expected();
        for path in [
            "/home/builder/.cache/paru/clone/evil/evil-1.0-1-x86_64.pkg.tar.zst",
            "/home/builder/.cache/paru/clone/hello/hello-extra-1.0-1-x86_64.pkg.tar.zst",
            "/home/builder/.cache/paru/clone/hello/hello-1.0-x86_64.pkg.tar.zst",
            "/home/builder/.cache/paru/clone/hello/hello--1-x86_64.pkg.tar.zst",
            "/home/builder/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.gz",
            "/home/builder/.cache/paru/clone/hello/hellox-1.0-1-x86_64.pkg.tar.zst",
            "/home/other/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst",
            "/tmp/hello-1.0-1-x86_64.pkg.tar.zst",
            "/home/builder/Downloads/hello-1.0-1-x86_64.pkg.tar.zst",
        ] {
            assert_eq!(expected.check(&built(&[path])), Err(Refusal::NotThisBuild), "{path}");
        }
        let good = "/home/builder/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst";
        let bad = "/home/builder/.cache/paru/clone/evil/evil-1.0-1-x86_64.pkg.tar.zst";
        assert_eq!(expected.check(&built(&[good, bad])), Err(Refusal::NotThisBuild), "all of them or none");
    }

    #[test]
    fn dependencies_are_installed_marked_and_removed_only_from_the_list() {
        let expected = expected();
        for request in [
            Request::Install(owned(&["scdoc"])),
            Request::MarkDeps(owned(&["scdoc", "libgreet"])),
            Request::MarkDeps(owned(&["hello-debug"])),
            Request::Remove(owned(&["scdoc"])),
            Request::MarkExplicit(owned(&["hello"])),
        ] {
            assert_eq!(expected.check(&request), Ok(()), "{request}");
        }
        for request in [
            Request::Install(owned(&["scdoc", "openssh"])),
            Request::MarkDeps(owned(&["hello"])),
            Request::MarkDeps(owned(&["scdoc-debug"])),
            Request::Remove(owned(&["hello-debug"])),
            Request::Remove(owned(&["glibc"])),
            Request::Remove(owned(&["hello"])),
            Request::MarkExplicit(owned(&["scdoc"])),
            Request::Upgrade,
            Request::UpgradeInstall(owned(&["scdoc"])),
            Request::RemoveOrphans(owned(&["scdoc"])),
            Request::Timer(true),
            Request::FlatpakSystemRemove(owned(&["org.gimp.GIMP"])),
        ] {
            assert_eq!(expected.check(&request), Err(Refusal::NotThisBuild), "{request}");
        }
    }
}

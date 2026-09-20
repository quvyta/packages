//! What an AUR build installs, worked out before the user confirms it.
//!
//! The AUR's records name each package's dependencies. Those something installed already
//! satisfies are left alone (`pacman -T`); those a repository has are installed from it; the rest
//! must come from the AUR and are built first, their own dependencies followed the same way.
//! paru and yay decide the same, so the confirmation lists what they will build and install.

use crate::catalog::aur::AurPackage;
use crate::pacman::Plan as RepoPlan;

/// How many rounds of dependencies are followed before the build is given up as too deep: far
/// more than any real package needs, and a stop for a loop the AUR's records might make.
const MAX_ROUNDS: usize = 32;

/// One package the build makes from the AUR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// The package name.
    pub name: String,
    /// The package base whose recipe makes it; the recipe that is reviewed.
    pub base: String,
    /// The version the AUR has, including the release.
    pub version: String,
    /// The base's maintainer as the AUR names it, `None` when it is orphaned: the review needs
    /// it to tell that a package changed hands.
    pub maintainer: Option<String>,
}

/// Everything an AUR build does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// The packages the user asked for.
    pub targets: Vec<String>,
    /// The packages built from the AUR, dependencies before what needs them, the targets last.
    pub builds: Vec<Built>,
    /// The dependencies installed from the repositories, as pacman would install them.
    pub repo: RepoPlan,
}

/// Why no plan could be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A package the user asked for is not in the AUR.
    NotInAur(String),
    /// A dependency is neither installed nor in a repository nor in the AUR.
    Missing(String),
    /// pacman could not answer a question; what it said.
    Query(String),
    /// The AUR could not be asked or its answer read.
    AurUnanswered,
    /// The dependencies go deeper than any real package's.
    TooDeep,
}

/// What `pacman -S --print` said about dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoAnswer {
    /// The repositories have all of them; this is what installing them would do.
    Plan(RepoPlan),
    /// The repositories lack these, as pacman named them.
    NotFound(Vec<String>),
}

/// The questions a plan asks of the machine and of the AUR.
pub trait Lookup {
    /// The dependencies among `deps` nothing installed satisfies.
    ///
    /// # Errors
    ///
    /// Returns what went wrong when the question could not be answered.
    fn unsatisfied(&self, deps: &[String]) -> Result<Vec<String>, String>;

    /// What installing `deps` from the repositories would do.
    ///
    /// # Errors
    ///
    /// Returns what went wrong when the question could not be answered.
    fn repositories(&self, deps: &[String]) -> Result<RepoAnswer, String>;

    /// The AUR's records of `names`; a name the AUR does not know is missing from the answer.
    /// `None` when the AUR could not be asked.
    fn aur(&self, names: &[String]) -> Option<Vec<AurPackage>>;
}

/// Works out what building `targets` from the AUR does.
///
/// # Errors
///
/// Returns why no plan could be made; nothing is built then.
pub fn resolve(targets: &[String], lookup: &dyn Lookup) -> Result<Plan, Problem> {
    let mut rounds: Vec<Vec<Built>> = Vec::new();
    let mut planned: Vec<String> = Vec::new();
    let mut from_repositories: Vec<String> = Vec::new();
    let mut pending = targets.to_vec();
    while !pending.is_empty() {
        if rounds.len() == MAX_ROUNDS {
            return Err(Problem::TooDeep);
        }
        let found = lookup.aur(&pending).ok_or(Problem::AurUnanswered)?;
        let mut round = Vec::new();
        let mut deps: Vec<String> = Vec::new();
        for name in &pending {
            let Some(package) = found.iter().find(|package| package.name == *name) else {
                let missing = name.clone();
                return Err(if rounds.is_empty() { Problem::NotInAur(missing) } else { Problem::Missing(missing) });
            };
            round.push(Built {
                name: name.clone(),
                base: package.base.clone(),
                version: package.version.clone(),
                maintainer: package.maintainer.clone(),
            });
            planned.push(name.clone());
            let all = package.depends.iter().chain(&package.make_depends).chain(&package.check_depends);
            for dep in all {
                let seen = planned.iter().any(|name| name == dep_name(dep)) || from_repositories.contains(dep);
                if !seen && !deps.contains(dep) {
                    deps.push(dep.clone());
                }
            }
        }
        rounds.push(round);
        let unsatisfied =
            if deps.is_empty() { Vec::new() } else { lookup.unsatisfied(&deps).map_err(Problem::Query)? };
        let (repository, aur) = split(unsatisfied, lookup)?;
        from_repositories.extend(repository);
        pending = Vec::new();
        for dep in aur {
            let name = dep_name(&dep).to_owned();
            if !planned.contains(&name) && !pending.contains(&name) {
                pending.push(name);
            }
        }
        // A dependency of one package may be a target or another dependency built later on;
        // it is then built once, where it is first needed.
        pending.retain(|name| !planned.contains(name));
    }
    let repo = if from_repositories.is_empty() {
        RepoPlan::default()
    } else {
        match lookup.repositories(&from_repositories).map_err(Problem::Query)? {
            RepoAnswer::Plan(plan) => plan,
            RepoAnswer::NotFound(missing) => {
                return Err(Problem::Missing(missing.into_iter().next().unwrap_or_default()));
            }
        }
    };
    let builds = rounds.into_iter().rev().flatten().collect();
    Ok(Plan { targets: targets.to_vec(), builds, repo })
}

/// Splits `deps` into those the repositories have and those they lack, asking again without the
/// missing ones until pacman has all the rest.
fn split(mut deps: Vec<String>, lookup: &dyn Lookup) -> Result<(Vec<String>, Vec<String>), Problem> {
    let mut lacking = Vec::new();
    while !deps.is_empty() {
        match lookup.repositories(&deps).map_err(Problem::Query)? {
            RepoAnswer::Plan(_) => break,
            RepoAnswer::NotFound(missing) => {
                let before = deps.len();
                let (gone, kept): (Vec<String>, Vec<String>) = deps.into_iter().partition(|dep| missing.contains(dep));
                if kept.len() == before {
                    // pacman named something it was not asked about; asking again would not end.
                    return Err(Problem::Query(missing.join(" ")));
                }
                lacking.extend(gone);
                deps = kept;
            }
        }
    }
    Ok((deps, lacking))
}

/// The package name of a dependency, without the version it asks for: `python` of
/// `python>=3.12`.
#[must_use]
pub fn dep_name(dep: &str) -> &str {
    dep.split(['<', '>', '=']).next().unwrap_or(dep)
}

/// The arguments that ask pacman which of `deps` nothing installed satisfies: `-T`, read by
/// [`read_unsatisfied`].
#[must_use]
pub fn check_args(deps: &[String]) -> Vec<String> {
    ["-T", "--"].iter().map(|arg| (*arg).to_owned()).chain(deps.iter().cloned()).collect()
}

/// Reads `pacman -T`: exit code 0 when everything is satisfied, 127 with one unsatisfied
/// dependency per line otherwise. `None` for any other ending.
#[must_use]
pub fn read_unsatisfied(code: Option<i32>, stdout: &str) -> Option<Vec<String>> {
    match code {
        Some(0) => Some(Vec::new()),
        Some(127) => Some(stdout.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect()),
        _ => None,
    }
}

/// The targets pacman said it could not find, from its error stream in the C locale:
/// `error: target not found: <target>`.
#[must_use]
pub fn not_found(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix("error: target not found: "))
        .map(|target| target.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::pacman::{Step, parse_install_plan};

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// A pretend machine: what is installed, what the repositories have, what the AUR has, and
    /// every question asked.
    #[derive(Default)]
    struct Machine {
        installed: Vec<&'static str>,
        repositories: Vec<&'static str>,
        aur: Vec<AurPackage>,
        offline: bool,
        asked: RefCell<Vec<String>>,
    }

    impl Machine {
        fn package(mut self, name: &str, depends: &[&str], make: &[&str], check: &[&str]) -> Self {
            self.aur.push(AurPackage {
                name: name.to_owned(),
                base: name.to_owned(),
                version: "1.0-1".to_owned(),
                depends: owned(depends),
                make_depends: owned(make),
                check_depends: owned(check),
                ..AurPackage::default()
            });
            self
        }
    }

    impl Lookup for Machine {
        fn unsatisfied(&self, deps: &[String]) -> Result<Vec<String>, String> {
            self.asked.borrow_mut().push(format!("-T {}", deps.join(" ")));
            Ok(deps.iter().filter(|dep| !self.installed.contains(&dep_name(dep))).cloned().collect())
        }

        fn repositories(&self, deps: &[String]) -> Result<RepoAnswer, String> {
            self.asked.borrow_mut().push(format!("-S {}", deps.join(" ")));
            let missing: Vec<String> =
                deps.iter().filter(|dep| !self.repositories.contains(&dep_name(dep))).cloned().collect();
            if !missing.is_empty() {
                return Ok(RepoAnswer::NotFound(missing));
            }
            let text: String = deps.iter().map(|dep| format!("extra|{}|1-1|100\n", dep_name(dep))).collect();
            Ok(RepoAnswer::Plan(parse_install_plan(&text)))
        }

        fn aur(&self, names: &[String]) -> Option<Vec<AurPackage>> {
            self.asked.borrow_mut().push(format!("aur {}", names.join(" ")));
            (!self.offline).then(|| self.aur.iter().filter(|package| names.contains(&package.name)).cloned().collect())
        }
    }

    fn names(plan: &Plan) -> (Vec<&str>, Vec<&str>) {
        let builds = plan.builds.iter().map(|build| build.name.as_str()).collect();
        let repo = plan.repo.steps.iter().map(|step| step.name.as_str()).collect();
        (builds, repo)
    }

    #[test]
    fn a_package_with_repository_dependencies_builds_alone() {
        let machine = Machine { installed: vec!["glibc"], repositories: vec!["scdoc", "glibc"], ..Machine::default() }
            .package("hello", &["glibc"], &["scdoc>=1.9"], &[]);
        let plan = resolve(&owned(&["hello"]), &machine).expect("a plan");
        assert_eq!(names(&plan), (vec!["hello"], vec!["scdoc"]));
        assert_eq!(plan.targets, ["hello"]);
        assert_eq!(
            plan.repo.steps[0],
            Step {
                repo: Some("extra".to_owned()),
                name: "scdoc".to_owned(),
                version: "1-1".to_owned(),
                size: Some(100)
            }
        );
        assert_eq!(*machine.asked.borrow(), ["aur hello", "-T glibc scdoc>=1.9", "-S scdoc>=1.9", "-S scdoc>=1.9"]);
    }

    #[test]
    fn aur_dependencies_are_built_first_and_followed_to_the_end() {
        let machine = Machine { repositories: vec!["cmake", "meson"], ..Machine::default() }
            .package("hello", &["libgreet>=2"], &["cmake"], &[])
            .package("libgreet", &["libcore"], &["meson"], &["hello"])
            .package("libcore", &[], &[], &[]);
        let plan = resolve(&owned(&["hello"]), &machine).expect("a plan");
        assert_eq!(names(&plan), (vec!["libcore", "libgreet", "hello"], vec!["cmake", "meson"]));
        assert_eq!(
            *machine.asked.borrow(),
            [
                "aur hello",
                "-T libgreet>=2 cmake",
                "-S libgreet>=2 cmake",
                "-S cmake",
                "aur libgreet",
                "-T libcore meson",
                "-S libcore meson",
                "-S meson",
                "aur libcore",
                "-S cmake meson",
            ],
            "a target another package needs is not asked about again"
        );
    }

    #[test]
    fn what_is_installed_already_is_left_alone() {
        let machine = Machine { installed: vec!["libgreet", "cmake"], ..Machine::default() }.package(
            "hello",
            &["libgreet"],
            &["cmake"],
            &[],
        );
        let plan = resolve(&owned(&["hello"]), &machine).expect("a plan");
        assert_eq!(names(&plan), (vec!["hello"], vec![]));
        assert_eq!(*machine.asked.borrow(), ["aur hello", "-T libgreet cmake"]);
    }

    #[test]
    fn a_target_or_dependency_nobody_has_is_said() {
        let machine = Machine::default().package("hello", &["nowhere"], &[], &[]);
        assert_eq!(resolve(&owned(&["hi"]), &machine), Err(Problem::NotInAur("hi".to_owned())));
        assert_eq!(resolve(&owned(&["hello"]), &machine), Err(Problem::Missing("nowhere".to_owned())));
        let offline = Machine { offline: true, ..Machine::default() };
        assert_eq!(resolve(&owned(&["hello"]), &offline), Err(Problem::AurUnanswered));
    }

    #[test]
    fn a_loop_in_the_records_ends() {
        let mut machine = Machine::default();
        for index in 0..=MAX_ROUNDS {
            let next = format!("p{}", index + 1);
            machine = machine.package(&format!("p{index}"), &[next.as_str()], &[], &[]);
        }
        assert_eq!(resolve(&owned(&["p0"]), &machine), Err(Problem::TooDeep));
    }

    #[test]
    fn pacman_answers_are_read() {
        assert_eq!(read_unsatisfied(Some(0), ""), Some(vec![]));
        assert_eq!(read_unsatisfied(Some(127), "scdoc>=1.9\nlibgreet\n"), Some(owned(&["scdoc>=1.9", "libgreet"])));
        assert_eq!(read_unsatisfied(Some(1), "error"), None);
        assert_eq!(read_unsatisfied(None, ""), None);
        let stderr = "error: target not found: libgreet\nerror: target not found: foo>=2\n";
        assert_eq!(not_found(stderr), ["libgreet", "foo>=2"]);
        assert_eq!(check_args(&owned(&["a>=1", "b"])), ["-T", "--", "a>=1", "b"]);
        assert_eq!(dep_name("python>=3.12"), "python");
        assert_eq!(dep_name("a=1"), "a");
        assert_eq!(dep_name("b<2"), "b");
        assert_eq!(dep_name("c"), "c");
    }
}

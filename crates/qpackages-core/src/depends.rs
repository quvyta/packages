//! Which AUR packages must be built to install some, and in what order.
//!
//! An AUR package can need others that only the AUR has, to run, to build or to test. Those are
//! built first, each before whatever needs it. The AUR builds by package base: one recipe can
//! make several packages, so the order is of bases, and a base is built once however many of
//! its packages are needed.
//!
//! What the repositories or the installed packages already satisfy is the caller's to say, and
//! so is the AUR's record of each name: the walk itself asks nothing of the network.

use std::fmt;

use crate::catalog::aur::AurPackage;
use crate::helper::is_package_name;

/// One base to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// The package base: the recipe's name, and the name of its repository.
    pub base: String,
    /// The packages of this base that are needed, in the order they were first needed.
    pub packages: Vec<String>,
    /// The base's maintainer, as the AUR names it; `None` when it is orphaned.
    pub maintainer: Option<String>,
}

/// Why the packages to build could not be worked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    /// Nothing satisfies `name`, and the AUR has no package by that name. `needed_by` is the
    /// package that needs it, `None` when it was asked for directly.
    NotFound {
        /// The name nothing satisfies.
        name: String,
        /// What needs it.
        needed_by: Option<String>,
    },
    /// The AUR names a base that does not follow pacman's rule for names, so no recipe is
    /// fetched for it.
    BadBase(String),
    /// These bases need each other, each the next and the last the first, so none can be built
    /// first.
    Cycle(Vec<String>),
}

impl fmt::Display for OrderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { name, needed_by: Some(by) } => {
                write!(formatter, "`{by}` needs `{name}`, which nothing provides")
            }
            Self::NotFound { name, needed_by: None } => write!(formatter, "the AUR has no package `{name}`"),
            Self::BadBase(base) => write!(formatter, "`{base}` is not a package base name"),
            Self::Cycle(bases) => write!(formatter, "these packages need each other: {}", bases.join(" → ")),
        }
    }
}

impl std::error::Error for OrderError {}

/// The name a dependency entry asks for, without its version condition: `obs-studio` of
/// `obs-studio>=28`.
#[must_use]
pub fn dependency_name(entry: &str) -> &str {
    entry.split(['<', '>', '=']).next().unwrap_or(entry).trim()
}

/// The bases to build to install `targets` from the AUR, each after every base it needs.
///
/// `lookup` gives the AUR's record of a name (an `info` answer, which carries the dependency
/// lists), and `satisfied` says whether a dependency is met without the AUR: by a repository
/// or by what is installed. The targets themselves are always built. Run, build and test
/// dependencies all count, since the build installs all three.
///
/// # Errors
///
/// [`OrderError::NotFound`] for a name neither satisfied nor in the AUR,
/// [`OrderError::BadBase`] for a base no recipe can be fetched for, and [`OrderError::Cycle`]
/// when bases need each other.
pub fn build_order<'a, S: AsRef<str>>(
    targets: &[S],
    lookup: impl Fn(&str) -> Option<&'a AurPackage>,
    satisfied: impl Fn(&str) -> bool,
) -> Result<Vec<Build>, OrderError> {
    // First every package that is needed, each once, in the order it was first needed.
    let mut needed: Vec<&AurPackage> = Vec::new();
    let mut queue: Vec<(String, Option<String>)> =
        targets.iter().map(|target| (target.as_ref().to_owned(), None)).collect();
    queue.reverse();
    while let Some((name, needed_by)) = queue.pop() {
        if needed.iter().any(|package| package.name == name) {
            continue;
        }
        let Some(package) = lookup(&name) else { return Err(OrderError::NotFound { name, needed_by }) };
        if !is_package_name(&package.base) {
            return Err(OrderError::BadBase(package.base.clone()));
        }
        needed.push(package);
        let wanted: Vec<(String, Option<String>)> = dependencies(package)
            .filter(|dependency| !satisfied(dependency))
            .map(|dependency| (dependency.to_owned(), Some(package.name.clone())))
            .collect();
        queue.extend(wanted.into_iter().rev());
    }

    // Then the bases, each after the bases its needed packages depend on.
    let mut builds: Vec<Build> = Vec::new();
    for package in &needed {
        match builds.iter_mut().find(|build| build.base == package.base) {
            Some(build) => build.packages.push(package.name.clone()),
            None => builds.push(Build {
                base: package.base.clone(),
                packages: vec![package.name.clone()],
                maintainer: package.maintainer.clone(),
            }),
        }
    }
    let base_of = |name: &str| needed.iter().find(|package| package.name == name).map(|package| package.base.as_str());
    let edges: Vec<Vec<usize>> = builds
        .iter()
        .map(|build| {
            let mut after = Vec::new();
            for package in needed.iter().filter(|package| package.base == build.base) {
                for dependency in dependencies(package).filter(|dependency| !satisfied(dependency)) {
                    let base = base_of(dependency).and_then(|base| builds.iter().position(|other| other.base == base));
                    if let Some(index) = base
                        && builds[index].base != build.base
                        && !after.contains(&index)
                    {
                        after.push(index);
                    }
                }
            }
            after
        })
        .collect();
    let order = topological(&edges)
        .map_err(|cycle| OrderError::Cycle(cycle.into_iter().map(|index| builds[index].base.clone()).collect()))?;
    let mut slots: Vec<Option<Build>> = builds.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|index| slots[index].take()).collect())
}

/// Every name `package` needs, in the order the AUR lists them: run, build, then test.
fn dependencies(package: &AurPackage) -> impl Iterator<Item = &str> {
    package
        .depends
        .iter()
        .chain(&package.make_depends)
        .chain(&package.check_depends)
        .map(|entry| dependency_name(entry))
}

/// The nodes in an order where each comes after every node it has an edge to, starting from
/// node 0; or, when there is none, the nodes of one cycle in edge order.
fn topological(edges: &[Vec<usize>]) -> Result<Vec<usize>, Vec<usize>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let mut marks = vec![Mark::New; edges.len()];
    let mut order = Vec::with_capacity(edges.len());
    for root in 0..edges.len() {
        if marks[root] != Mark::New {
            continue;
        }
        // An explicit stack of (node, next edge), so a long chain of dependencies cannot
        // overflow the thread's stack.
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        marks[root] = Mark::Open;
        while let Some(top) = stack.last_mut() {
            let node = top.0;
            if let Some(&to) = edges[node].get(top.1) {
                top.1 += 1;
                match marks[to] {
                    Mark::New => {
                        marks[to] = Mark::Open;
                        stack.push((to, 0));
                    }
                    Mark::Open => {
                        let from = stack.iter().position(|&(open, _)| open == to).unwrap_or(0);
                        return Err(stack[from..].iter().map(|&(open, _)| open).collect());
                    }
                    Mark::Done => {}
                }
            } else {
                marks[node] = Mark::Done;
                order.push(node);
                stack.pop();
            }
        }
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, base: &str, depends: &[&str], make: &[&str], check: &[&str]) -> AurPackage {
        let list = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect();
        AurPackage {
            name: name.to_owned(),
            base: base.to_owned(),
            version: "1.0-1".to_owned(),
            maintainer: Some("someone".to_owned()),
            depends: list(depends),
            make_depends: list(make),
            check_depends: list(check),
            ..AurPackage::default()
        }
    }

    /// The order for `targets` among `aur`, with `repo` standing for everything the
    /// repositories have.
    fn order(targets: &[&str], aur: &[AurPackage], repo: &[&str]) -> Result<Vec<(String, Vec<String>)>, OrderError> {
        let builds =
            build_order(targets, |name| aur.iter().find(|package| package.name == name), |name| repo.contains(&name))?;
        Ok(builds.into_iter().map(|build| (build.base, build.packages)).collect())
    }

    fn built(bases: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        bases
            .iter()
            .map(|(base, names)| ((*base).to_owned(), names.iter().map(|&name| name.to_owned()).collect()))
            .collect()
    }

    #[test]
    fn a_package_with_only_repository_dependencies_is_built_alone() {
        let aur = [package("hello", "hello", &["glibc"], &["gcc>=14"], &[])];
        assert_eq!(order(&["hello"], &aur, &["glibc", "gcc"]), Ok(built(&[("hello", &["hello"])])));
    }

    #[test]
    fn aur_dependencies_of_every_kind_come_first_transitively() {
        let aur = [
            package("app", "app", &["lib-a>=2"], &["tool-b"], &["test-c"]),
            package("lib-a", "lib-a", &["lib-d"], &[], &[]),
            package("tool-b", "tool-b", &["glibc"], &[], &[]),
            package("test-c", "test-c", &[], &[], &[]),
            package("lib-d", "lib-d", &[], &[], &[]),
        ];
        assert_eq!(
            order(&["app"], &aur, &["glibc"]),
            Ok(built(&[
                ("lib-d", &["lib-d"]),
                ("lib-a", &["lib-a"]),
                ("tool-b", &["tool-b"]),
                ("test-c", &["test-c"]),
                ("app", &["app"]),
            ]))
        );
    }

    #[test]
    fn what_is_satisfied_is_not_built_even_when_the_aur_has_it() {
        let aur = [package("app", "app", &["lib-a"], &[], &[]), package("lib-a", "lib-a", &[], &[], &[])];
        assert_eq!(order(&["app"], &aur, &["lib-a"]), Ok(built(&[("app", &["app"])])));
    }

    #[test]
    fn a_base_is_built_once_for_all_its_needed_packages() {
        let aur = [
            package("app", "app", &["suite-core", "suite-extra"], &[], &[]),
            package("suite-core", "suite", &[], &[], &[]),
            package("suite-extra", "suite", &["suite-core", "lib-x"], &[], &[]),
            package("lib-x", "lib-x", &[], &[], &[]),
        ];
        assert_eq!(
            order(&["app"], &aur, &[]),
            Ok(built(&[("lib-x", &["lib-x"]), ("suite", &["suite-core", "suite-extra"]), ("app", &["app"])])),
            "the split package's own dependency moves the whole base after it"
        );
    }

    #[test]
    fn a_shared_dependency_is_built_once() {
        let aur = [
            package("one", "one", &["lib"], &[], &[]),
            package("two", "two", &["lib"], &[], &[]),
            package("lib", "lib", &[], &[], &[]),
        ];
        assert_eq!(
            order(&["one", "two"], &aur, &[]),
            Ok(built(&[("lib", &["lib"]), ("one", &["one"]), ("two", &["two"])]))
        );
    }

    #[test]
    fn bases_that_need_each_other_are_a_cycle() {
        let aur = [
            package("app", "app", &["a"], &[], &[]),
            package("a", "a", &["b"], &[], &[]),
            package("b", "b", &[], &["a"], &[]),
        ];
        assert_eq!(order(&["app"], &aur, &[]), Err(OrderError::Cycle(vec!["a".to_owned(), "b".to_owned()])));
        let own = [package("self", "self", &["self"], &[], &[])];
        assert!(order(&["self"], &own, &[]).is_ok(), "a package that names itself needs nothing more");
    }

    #[test]
    fn a_name_nothing_provides_says_what_needed_it() {
        let aur = [package("app", "app", &["ghost>=1"], &[], &[])];
        assert_eq!(
            order(&["app"], &aur, &[]),
            Err(OrderError::NotFound { name: "ghost".to_owned(), needed_by: Some("app".to_owned()) })
        );
        assert_eq!(
            order(&["nothing"], &aur, &[]),
            Err(OrderError::NotFound { name: "nothing".to_owned(), needed_by: None })
        );
    }

    #[test]
    fn a_base_that_is_not_a_name_is_refused() {
        let aur = [package("app", "--upload-pack=x", &[], &[], &[])];
        assert_eq!(order(&["app"], &aur, &[]), Err(OrderError::BadBase("--upload-pack=x".to_owned())));
    }

    #[test]
    fn a_dependency_name_drops_its_version_condition() {
        assert_eq!(dependency_name("obs-studio>=28"), "obs-studio");
        assert_eq!(dependency_name("python<3.15"), "python");
        assert_eq!(dependency_name("libfoo.so=1-64"), "libfoo.so");
        assert_eq!(dependency_name("plain"), "plain");
    }

    #[test]
    fn a_long_chain_does_not_exhaust_the_stack() {
        let aur: Vec<AurPackage> = (0..5000)
            .map(|index| {
                let next = format!("p{}", index + 1);
                let depends: &[&str] = if index < 4999 { &[next.as_str()] } else { &[] };
                package(&format!("p{index}"), &format!("p{index}"), depends, &[], &[])
            })
            .collect();
        let builds = order(&["p0"], &aur, &[]).expect("ordered");
        assert_eq!(builds.len(), 5000);
        assert_eq!(builds[0].0, "p4999");
        assert_eq!(builds[4999].0, "p0");
    }
}

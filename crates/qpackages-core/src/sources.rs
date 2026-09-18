//! Finding out which package sources this machine has.
//!
//! Detection only asks whether a program exists on `PATH`; it never runs one. Running the
//! programs would be slow, could prompt, and is not needed to decide what the screen shows.

use std::env;
use std::path::PathBuf;

/// A place packages come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The official repositories, through `pacman`.
    Pacman,
    /// The Arch User Repository, through a helper.
    Aur,
    /// Flatpak.
    Flatpak,
    /// Snap.
    Snap,
}

/// Whether a source's program is on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// The tool is there and can be used.
    Ready {
        /// Where the program was found.
        program: PathBuf,
    },
    /// The tool is not installed. Not an error: the source shows faint and offers to install.
    Missing,
}

/// Which helper drives the AUR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AurHelper {
    /// `paru`.
    Paru,
    /// `yay`.
    Yay,
}

impl AurHelper {
    /// The program name to look for.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Paru => "paru",
            Self::Yay => "yay",
        }
    }
}

/// The user's preference when both helpers are installed.
///
/// A preference is only a tie-breaker: asking for one helper never makes the AUR unavailable
/// while the other helper would work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AurPreference {
    /// Take `paru` when it is there, otherwise `yay`.
    #[default]
    Auto,
    /// Prefer `paru`.
    Paru,
    /// Prefer `yay`.
    Yay,
}

impl AurPreference {
    /// The helpers in the order they should be tried.
    const fn order(self) -> [AurHelper; 2] {
        match self {
            Self::Auto | Self::Paru => [AurHelper::Paru, AurHelper::Yay],
            Self::Yay => [AurHelper::Yay, AurHelper::Paru],
        }
    }
}

/// What [`detect`] found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// `pacman`.
    pub pacman: Availability,
    /// The AUR helper chosen by [`AurPreference`] among those installed.
    pub aur: Availability,
    /// Which helper `aur` refers to; `None` when the AUR is missing.
    pub aur_helper: Option<AurHelper>,
    /// `flatpak`.
    pub flatpak: Availability,
    /// `snap`.
    pub snap: Availability,
    /// Whether `fakeroot` is installed. Checking for updates syncs a throwaway copy of the
    /// database without privileges, and that needs it.
    pub fakeroot: bool,
}

impl Sources {
    /// The availability of one source.
    #[must_use]
    pub const fn get(&self, source: Source) -> &Availability {
        match source {
            Source::Pacman => &self.pacman,
            Source::Aur => &self.aur,
            Source::Flatpak => &self.flatpak,
            Source::Snap => &self.snap,
        }
    }
}

/// Looks for each source's program. `lookup` answers whether a program exists and where, so
/// tests can describe a machine without having one; [`on_path`] is the real one.
#[must_use]
pub fn detect(preference: AurPreference, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> Sources {
    let find = |program: &str| lookup(program).map_or(Availability::Missing, |program| Availability::Ready { program });
    let (aur, aur_helper) = preference
        .order()
        .into_iter()
        .find_map(|helper| lookup(helper.program()).map(|program| (Availability::Ready { program }, Some(helper))))
        .unwrap_or((Availability::Missing, None));
    Sources {
        pacman: find("pacman"),
        aur,
        aur_helper,
        flatpak: find("flatpak"),
        snap: find("snap"),
        fakeroot: lookup("fakeroot").is_some(),
    }
}

/// Finds a program by searching the directories in `PATH`, the way a shell would.
///
/// Only a regular file counts: a directory that happens to carry the program's name is not
/// the program. An empty name never matches for the same reason.
#[must_use]
pub fn on_path(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A pretend machine: a program is found when its name is in the list.
    fn machine(programs: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| programs.contains(&name).then(|| Path::new("/usr/bin").join(name))
    }

    fn ready(program: &str) -> Availability {
        Availability::Ready { program: Path::new("/usr/bin").join(program) }
    }

    #[test]
    fn a_machine_with_everything() {
        let sources = detect(AurPreference::Auto, &machine(&["pacman", "paru", "yay", "flatpak", "snap", "fakeroot"]));
        assert_eq!(sources.pacman, ready("pacman"));
        assert_eq!(sources.aur, ready("paru"), "paru wins when nothing is preferred");
        assert_eq!(sources.aur_helper, Some(AurHelper::Paru));
        assert_eq!(sources.flatpak, ready("flatpak"));
        assert_eq!(sources.snap, ready("snap"));
        assert!(sources.fakeroot);
        assert_eq!(sources.get(Source::Aur), &ready("paru"));
    }

    #[test]
    fn a_machine_with_only_pacman() {
        let sources = detect(AurPreference::Auto, &machine(&["pacman"]));
        assert_eq!(sources.pacman, ready("pacman"));
        assert_eq!(sources.aur, Availability::Missing);
        assert_eq!(sources.aur_helper, None);
        assert_eq!(sources.flatpak, Availability::Missing);
        assert_eq!(sources.snap, Availability::Missing, "a missing snap is a state, not an error");
        assert!(!sources.fakeroot);
    }

    #[test]
    fn paru_without_yay_drives_the_aur_whatever_the_preference() {
        for preference in [AurPreference::Auto, AurPreference::Paru, AurPreference::Yay] {
            let sources = detect(preference, &machine(&["pacman", "paru", "fakeroot"]));
            assert_eq!(sources.aur, ready("paru"), "{preference:?}");
            assert_eq!(sources.aur_helper, Some(AurHelper::Paru), "{preference:?}");
            assert_eq!(sources.snap, Availability::Missing);
        }
    }

    #[test]
    fn both_helpers_follow_the_preference() {
        let both = machine(&["pacman", "paru", "yay"]);
        assert_eq!(detect(AurPreference::Auto, &both).aur_helper, Some(AurHelper::Paru));
        assert_eq!(detect(AurPreference::Paru, &both).aur_helper, Some(AurHelper::Paru));
        let yay = detect(AurPreference::Yay, &both);
        assert_eq!(yay.aur_helper, Some(AurHelper::Yay));
        assert_eq!(yay.aur, ready("yay"));
    }

    #[test]
    fn only_yay_is_used_even_when_paru_is_preferred() {
        let sources = detect(AurPreference::Paru, &machine(&["pacman", "yay"]));
        assert_eq!(sources.aur_helper, Some(AurHelper::Yay));
        assert_eq!(sources.aur, ready("yay"));
    }

    #[test]
    fn on_path_finds_sh_and_not_a_made_up_program() {
        let sh = on_path("sh").expect("every Unix machine has sh");
        assert!(sh == Path::new("/usr/bin/sh") || sh == Path::new("/bin/sh"), "{}", sh.display());
        assert_eq!(on_path("definitely-not-a-program-xyz"), None);
        assert_eq!(on_path(""), None, "an empty name is nothing, not a directory");
    }
}

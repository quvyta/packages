//! What a pacman transaction would do, read before doing it.
//!
//! The user sees the full list of what an install or a removal will touch and confirms it before
//! anything runs. pacman gives that list without privileges through `--print-format`, and the
//! format is ours to choose: `|` separates the fields only inside that machine format asked of
//! pacman; it is never drawn.

/// One package a transaction would install, upgrade or remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The repository the package comes from. A removal has none.
    pub repo: Option<String>,
    /// The package name.
    pub name: String,
    /// The version the transaction leaves or takes away, including the release.
    pub version: String,
    /// Download size in bytes, when the format asked for it and pacman gave a number.
    pub size: Option<u64>,
}

/// Everything a transaction would do, in the order pacman would do it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// The packages the transaction touches.
    pub steps: Vec<Step>,
}

impl Plan {
    /// The bytes to download, summing every step that knows its size.
    ///
    /// A step without a size adds nothing, so the total is a floor rather than an exact figure
    /// when pacman could not give a number.
    #[must_use]
    pub fn download(&self) -> u64 {
        self.steps.iter().filter_map(|step| step.size).sum()
    }
}

/// Reads `pacman -S --print-format '%r|%n|%v|%s'`, one `repo|name|version|size` line per package.
///
/// A line with a different number of fields is not a step and is skipped; a size that is not a
/// number is dropped and the step is kept.
#[must_use]
pub fn parse_install_plan(text: &str) -> Plan {
    let steps = text
        .lines()
        .filter_map(|line| match line.split('|').collect::<Vec<_>>()[..] {
            [repo, name, version, size] => Some(Step {
                repo: Some(repo.to_owned()),
                name: name.to_owned(),
                version: version.to_owned(),
                size: size.parse().ok(),
            }),
            _ => None,
        })
        .collect();
    Plan { steps }
}

/// Reads `pacman -Rns --print-format '%n|%v'`, one `name|version` line per package, the
/// dependencies that go with it included.
///
/// A line with a different number of fields is not a step and is skipped.
#[must_use]
pub fn parse_remove_plan(text: &str) -> Plan {
    let steps = text
        .lines()
        .filter_map(|line| match line.split('|').collect::<Vec<_>>()[..] {
            [name, version] => {
                Some(Step { repo: None, name: name.to_owned(), version: version.to_owned(), size: None })
            }
            _ => None,
        })
        .collect();
    Plan { steps }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTALL_GIMP: &str = include_str!("../../tests/fixtures/pacman-print-install-gimp.txt");
    const REMOVE_YAY: &str = include_str!("../../tests/fixtures/pacman-print-remove-yay.txt");

    #[test]
    fn reads_the_real_install_plan_of_gimp() {
        let plan = parse_install_plan(INSTALL_GIMP);
        assert_eq!(plan.steps.len(), 9, "gimp and the eight dependencies it pulls in");
        assert_eq!(
            plan.steps[8],
            Step {
                repo: Some("extra".to_owned()),
                name: "gimp".to_owned(),
                version: "3.2.6-1".to_owned(),
                size: Some(24_907_325),
            }
        );
        assert_eq!(plan.steps[1].version, "1:0.3.4-6", "an epoch stays in the version");
        assert_eq!(plan.download(), 41_800_275, "the exact sum of the nine sizes in the file");
    }

    #[test]
    fn reads_the_real_remove_plan_of_yay() {
        let plan = parse_remove_plan(REMOVE_YAY);
        assert_eq!(
            plan.steps,
            [Step { repo: None, name: "yay".to_owned(), version: "13.0.1-1".to_owned(), size: None }],
            "a removal has no repository and nothing to download"
        );
        assert_eq!(plan.download(), 0);
    }

    #[test]
    fn a_line_with_the_wrong_number_of_fields_is_skipped() {
        let text =
            "extra|babl|0.1.128-1|1608538\nresolving dependencies...\nextra|gegl\nextra|gimp|3.2.6-1|24907325\n\n";
        let plan = parse_install_plan(text);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[1].name, "gimp");
        assert_eq!(parse_remove_plan("yay|13.0.1-1\nyay\n").steps.len(), 1);
    }

    #[test]
    fn a_size_that_is_not_a_number_is_dropped_and_the_step_survives() {
        let plan = parse_install_plan("extra|babl|0.1.128-1|unknown\nextra|gegl|0.4.72-2|4544681\n");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].size, None);
        assert_eq!(plan.download(), 4_544_681, "a step without a size adds nothing to the total");
    }

    #[test]
    fn empty_text_is_an_empty_plan() {
        assert_eq!(parse_install_plan(""), Plan::default());
        assert_eq!(parse_remove_plan(""), Plan::default());
    }
}

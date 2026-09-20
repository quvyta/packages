//! The plan's questions asked of this machine: pacman, without privileges, in the C locale so its
//! answers can be read, and the AUR's RPC.

use qpackages_core::build::Plan;
use qpackages_core::build::plan::{self, Lookup, Problem, RepoAnswer, check_args, not_found, read_unsatisfied};
use qpackages_core::catalog::aur::AurPackage;
use qpackages_core::pacman::command::{PACMAN, parsed_env, print_install};
use qpackages_core::pacman::parse_install_plan;

use crate::runner::Runner;
use crate::store::data::aur_info;

/// Asks through a runner.
struct Asking<'a>(&'a dyn Runner);

impl Lookup for Asking<'_> {
    fn unsatisfied(&self, deps: &[String]) -> Result<Vec<String>, String> {
        let output = self.0.output(PACMAN, &check_args(deps), &parsed_env()).map_err(|error| error.to_string())?;
        read_unsatisfied(output.code, &output.stdout).ok_or_else(|| said(&output.stderr, &output.stdout))
    }

    fn repositories(&self, deps: &[String]) -> Result<RepoAnswer, String> {
        let output = self.0.output(PACMAN, &print_install(deps), &parsed_env()).map_err(|error| error.to_string())?;
        if output.succeeded() {
            return Ok(RepoAnswer::Plan(parse_install_plan(&output.stdout)));
        }
        let missing = not_found(&output.stderr);
        if missing.is_empty() { Err(said(&output.stderr, &output.stdout)) } else { Ok(RepoAnswer::NotFound(missing)) }
    }

    fn aur(&self, names: &[String]) -> Option<Vec<AurPackage>> {
        aur_info(self.0, names).ok()
    }
}

/// What pacman said, its error stream first, on one line.
fn said(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    text.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Works out what building `targets` from the AUR does on this machine. Runs in the background.
///
/// # Errors
///
/// Returns why no plan could be made.
pub fn plan(runner: &dyn Runner, targets: &[String]) -> Result<Plan, Problem> {
    plan::resolve(targets, &Asking(runner))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use qpackages_core::catalog::aur::info_urls;
    use qpackages_core::catalog::net::{CURL, curl_args};

    use super::*;
    use crate::runner::Recorded;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../qpackages-core/tests/fixtures/build").join(name);
        std::fs::read_to_string(path).expect("the fixture is readable")
    }

    /// cbonsai's record and pacman's answers as they were on a fresh Arch machine.
    fn recorded() -> Recorded {
        let recorded = Recorded::default();
        let url = info_urls(&["cbonsai"]).remove(0);
        recorded.answer(CURL, &curl_args(&url), &fixture("aur-info-cbonsai.json"), 0);
        let deps = ["gcc", "ncurses", "scdoc"].map(str::to_owned);
        recorded.answer(PACMAN, &check_args(&deps), &fixture("pacman-check-cbonsai.txt"), 127);
        recorded
    }

    #[test]
    fn pacman_and_the_aur_are_asked_in_the_c_locale_and_their_answers_read() {
        let recorded = recorded();
        recorded.answer(PACMAN, &print_install(&["scdoc"]), &fixture("pacman-print-scdoc.txt"), 0);
        let plan = plan(&recorded, &["cbonsai".to_owned()]).expect("a plan");
        assert_eq!(plan.builds.iter().map(|build| build.name.as_str()).collect::<Vec<_>>(), ["cbonsai"]);
        assert_eq!(plan.repo.steps[0].name, "scdoc");
        assert_eq!(plan.repo.steps[0].size, Some(15_525));
        let pacman: Vec<_> = recorded.calls().into_iter().filter(|call| call.program == PACMAN).collect();
        assert!(
            pacman
                .iter()
                .all(|call| call.env == [("LC_ALL".to_owned(), "C".to_owned()), ("LANG".to_owned(), "C".to_owned())])
        );
    }

    #[test]
    fn a_dependency_no_repository_has_is_looked_for_in_the_aur() {
        let recorded = recorded();
        let stderr = fixture("pacman-print-not-found.txt");
        recorded.fail(PACMAN, &print_install(&["scdoc"]), &stderr, 1);
        let url = info_urls(&["scdoc"]).remove(0);
        recorded.answer(CURL, &curl_args(&url), &fixture("aur-info-none.json"), 0);
        assert_eq!(plan(&recorded, &["cbonsai".to_owned()]), Err(Problem::Missing("scdoc".to_owned())));
    }

    #[test]
    fn a_question_pacman_cannot_answer_is_said_in_its_words() {
        let recorded = recorded();
        recorded.fail(
            PACMAN,
            &print_install(&["scdoc"]),
            "error: failed to init transaction (unable to lock database)\n",
            1,
        );
        assert_eq!(
            plan(&recorded, &["cbonsai".to_owned()]),
            Err(Problem::Query("error: failed to init transaction (unable to lock database)".to_owned()))
        );
        let offline = Recorded::default();
        assert_eq!(plan(&offline, &["cbonsai".to_owned()]), Err(Problem::AurUnanswered));
    }
}

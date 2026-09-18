//! The updates waiting for the system, read from `pacman -Qu` and the AUR helpers.

/// One package that has a newer version available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The package name.
    pub name: String,
    /// The version installed now.
    pub from: String,
    /// The version waiting in the repositories.
    pub to: String,
    /// True when pacman marked the line `[ignored]`: the package is in `IgnorePkg`, so a plain
    /// upgrade will leave it where it is.
    pub ignored: bool,
}

/// Reads the lines of `pacman -Qu`, `paru -Qua` and `yay -Qua`: `name old -> new`, with an
/// optional trailing field.
///
/// The three commands share the shape because the helpers reproduce pacman's format. The
/// trailing field is `[ignored]` from pacman and an age such as `[1d1h]` from yay; only the
/// first one carries meaning. A line that does not fit the shape, such as a warning that was
/// mixed into the stream, is skipped.
#[must_use]
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines().filter_map(parse_line).collect()
}

/// Reads one `name old -> new [mark]` line, or `None` when the line is something else.
fn parse_line(line: &str) -> Option<Update> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    let from = words.next()?;
    if words.next()? != "->" {
        return None;
    }
    let to = words.next()?;
    let ignored = words.next() == Some("[ignored]");
    Some(Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored })
}

/// What a finished `pacman -Qu` meant.
///
/// pacman exits 1 both when there is nothing to update and when something went wrong, so the
/// exit code alone cannot tell an up-to-date system from a broken one; the streams decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The check ran; the list is empty when the system is up to date.
    Updates(Vec<Update>),
    /// The check did not run to the end.
    Failed {
        /// What pacman wrote on stderr, for a diagnostic rather than for the user.
        stderr: String,
    },
}

/// Turns the exit status and streams of `pacman -Qu` into what they meant.
///
/// Exit 0 always parses the output. Exit 1 with both streams empty is a system with nothing to
/// update, which is the ordinary case and not a failure. Exit 1 with something on stderr, any
/// other code, or no code at all (the process was killed by a signal) is a failure.
#[must_use]
pub fn read_update_check(code: Option<i32>, stdout: &str, stderr: &str) -> UpdateCheck {
    match code {
        Some(0) => UpdateCheck::Updates(parse_updates(stdout)),
        Some(1) if stdout.is_empty() && stderr.is_empty() => UpdateCheck::Updates(Vec::new()),
        _ => UpdateCheck::Failed { stderr: stderr.to_owned() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARU: &str = include_str!("../../tests/fixtures/paru-qua.txt");
    const YAY: &str = include_str!("../../tests/fixtures/yay-qua.txt");

    fn update(name: &str, from: &str, to: &str) -> Update {
        Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
    }

    #[test]
    fn reads_the_real_output_of_paru() {
        let updates = parse_updates(PARU);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0], update("google-chrome", "140.0.7339.127-1", "140.0.7339.185-1"));
        assert_eq!(updates[1], update("brave-bin", "1:1.95.101-1", "1:1.95.102-1"), "an epoch stays in the version");
    }

    #[test]
    fn the_age_field_yay_appends_is_ignored() {
        let from_yay = parse_updates(YAY);
        let from_paru = parse_updates(PARU);
        assert_eq!(from_yay, from_paru, "the same updates, whatever yay adds after them");
        assert!(from_yay.iter().all(|update| !update.ignored), "an age is not an ignore mark");
    }

    #[test]
    fn a_package_pacman_marks_ignored_is_flagged() {
        let updates = parse_updates("linux 6.18.1-1 -> 6.18.2-1 [ignored]\nbash 5.3.15-1 -> 5.3.16-1\n");
        assert_eq!(updates.len(), 2);
        assert!(updates[0].ignored);
        assert_eq!(updates[0].to, "6.18.2-1", "the mark does not leak into the version");
        assert!(!updates[1].ignored);
    }

    #[test]
    fn a_warning_line_and_an_empty_line_are_not_updates() {
        let text = "warning: database file for 'core' does not exist (use '-Sy' to download)\n\n";
        assert!(parse_updates(text).is_empty(), "warnings belong to stderr but may be mixed in");
        assert!(parse_updates("").is_empty());
    }

    #[test]
    fn exit_one_with_silent_streams_means_nothing_to_update() {
        match read_update_check(Some(1), "", "") {
            UpdateCheck::Updates(updates) => assert!(updates.is_empty()),
            UpdateCheck::Failed { stderr } => panic!("an up-to-date system is not a failure: {stderr}"),
        }
    }

    #[test]
    fn exit_one_with_something_on_stderr_is_a_failure() {
        let stderr = "error: could not open file /var/lib/pacman/local: Permission denied\n";
        assert_eq!(read_update_check(Some(1), "", stderr), UpdateCheck::Failed { stderr: stderr.to_owned() });
    }

    #[test]
    fn exit_zero_parses_the_output() {
        match read_update_check(Some(0), PARU, "") {
            UpdateCheck::Updates(updates) => assert_eq!(updates.len(), 2),
            UpdateCheck::Failed { stderr } => panic!("exit 0 is never a failure: {stderr}"),
        }
    }

    #[test]
    fn a_process_killed_by_a_signal_is_a_failure_even_when_silent() {
        assert!(matches!(read_update_check(None, "", ""), UpdateCheck::Failed { .. }));
    }
}

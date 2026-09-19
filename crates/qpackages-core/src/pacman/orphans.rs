//! The orphans: packages installed as dependencies that nothing needs any more.

use crate::helper::is_package_name;

/// Reads a finished `pacman -Qtdq`: the orphans' names, sorted and without repeats.
///
/// pacman exits 1 both when there are no orphans and when something went wrong; silent
/// streams tell the first. A line that is not a package name is skipped, so a warning mixed
/// into the stream never becomes a name to remove.
///
/// # Errors
///
/// Returns what pacman wrote on stderr when the query did not run to its end.
pub fn read_orphans(code: Option<i32>, stdout: &str, stderr: &str) -> Result<Vec<String>, String> {
    match code {
        Some(0) => {
            let mut names: Vec<String> =
                stdout.lines().map(str::trim).filter(|name| is_package_name(name)).map(str::to_owned).collect();
            names.sort();
            names.dedup();
            Ok(names)
        }
        Some(1) if stdout.trim().is_empty() && stderr.trim().is_empty() => Ok(Vec::new()),
        _ => Err(stderr.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_come_back_sorted_and_once() {
        let names = read_orphans(Some(0), "python-wheel\nlibfoo\n\npython-wheel\n", "");
        assert_eq!(names, Ok(vec!["libfoo".to_owned(), "python-wheel".to_owned()]));
    }

    #[test]
    fn exit_one_with_silent_streams_means_no_orphans() {
        assert_eq!(read_orphans(Some(1), "", ""), Ok(Vec::new()));
    }

    #[test]
    fn a_failure_is_not_an_empty_list() {
        let stderr = "error: could not open file /var/lib/pacman/local/ALPM_DB_VERSION\n";
        assert_eq!(read_orphans(Some(1), "", stderr), Err(stderr.to_owned()));
        assert_eq!(read_orphans(None, "", ""), Err(String::new()));
        assert_eq!(read_orphans(Some(2), "libfoo\n", ""), Err(String::new()));
    }

    #[test]
    fn a_line_that_is_not_a_name_is_never_a_name_to_remove() {
        let names = read_orphans(Some(0), "warning: something\n--cascade\nlibfoo\n", "");
        assert_eq!(names, Ok(vec!["libfoo".to_owned()]));
    }
}

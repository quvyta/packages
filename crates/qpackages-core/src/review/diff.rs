//! The changed lines between two versions of a recipe file.

/// What happened to one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The line is in both versions.
    Same,
    /// The line is only in the new version.
    Added,
    /// The line is only in the old version.
    Removed,
}

/// One line of a difference, with its number in each version it is in, counted from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// What happened to it.
    pub kind: Kind,
    /// The text, without its line ending.
    pub text: String,
    /// Its number in the old version, when it is there.
    pub old: Option<usize>,
    /// Its number in the new version, when it is there.
    pub new: Option<usize>,
}

/// The largest table the line matching builds, in cells. A recipe is a few hundred lines; past
/// this, two files have so little to do with each other that showing all of the old one removed
/// and all of the new one added says the same thing without the cost.
const MAX_CELLS: usize = 4_000_000;

/// Every line of `old` and `new` in order, marked the same, added or removed, keeping as many
/// lines the same as possible.
#[must_use]
pub fn diff_lines(old: &str, new: &str) -> Vec<Line> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let (rows, cols) = (old.len(), new.len());
    let fits = (rows + 1).checked_mul(cols + 1).is_some_and(|cells| cells <= MAX_CELLS);
    // `common[i][j]` is how many lines `old[i..]` and `new[j..]` can keep in common.
    let mut common = vec![0_u32; if fits { (rows + 1) * (cols + 1) } else { 0 }];
    let at = |i: usize, j: usize| i * (cols + 1) + j;
    if fits {
        for i in (0..rows).rev() {
            for j in (0..cols).rev() {
                common[at(i, j)] = if old[i] == new[j] {
                    common[at(i + 1, j + 1)] + 1
                } else {
                    common[at(i + 1, j)].max(common[at(i, j + 1)])
                };
            }
        }
    }
    let mut lines = Vec::with_capacity(rows.max(cols));
    let (mut i, mut j) = (0, 0);
    while i < rows || j < cols {
        let line = |kind, text: &str, old, new| Line { kind, text: text.to_owned(), old, new };
        if fits && i < rows && j < cols && old[i] == new[j] {
            lines.push(line(Kind::Same, old[i], Some(i + 1), Some(j + 1)));
            (i, j) = (i + 1, j + 1);
        } else if i < rows && (j == cols || !fits || common[at(i + 1, j)] >= common[at(i, j + 1)]) {
            lines.push(line(Kind::Removed, old[i], Some(i + 1), None));
            i += 1;
        } else {
            lines.push(line(Kind::Added, new[j], None, Some(j + 1)));
            j += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                let sign = match line.kind {
                    Kind::Same => ' ',
                    Kind::Added => '+',
                    Kind::Removed => '-',
                };
                format!("{sign}{}", line.text)
            })
            .collect()
    }

    #[test]
    fn a_changed_line_is_one_removed_and_one_added_between_kept_ones() {
        let lines = diff_lines("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(marked(&lines), [" a", "-b", "+B", " c"]);
        assert_eq!((lines[1].old, lines[1].new), (Some(2), None));
        assert_eq!((lines[2].old, lines[2].new), (None, Some(2)));
        assert_eq!((lines[3].old, lines[3].new), (Some(3), Some(3)));
    }

    #[test]
    fn insertions_and_deletions_keep_the_rest_in_common() {
        assert_eq!(marked(&diff_lines("a\nc\n", "a\nb\nc\nd\n")), [" a", "+b", " c", "+d"]);
        assert_eq!(marked(&diff_lines("x\na\ny\n", "a\n")), ["-x", " a", "-y"]);
        assert_eq!(marked(&diff_lines("", "a\n")), ["+a"]);
        assert_eq!(marked(&diff_lines("a\n", "")), ["-a"]);
        assert!(diff_lines("", "").is_empty());
    }

    #[test]
    fn files_too_large_to_match_are_all_removed_then_all_added() {
        let old = "a\n".repeat(2100);
        let new = "a\n".repeat(2100);
        let lines = diff_lines(&old, &new);
        assert_eq!(lines.len(), 4200);
        assert!(lines[..2100].iter().all(|line| line.kind == Kind::Removed));
        assert!(lines[2100..].iter().all(|line| line.kind == Kind::Added));
    }
}

//! pacman's version order, for telling whether the AUR holds a newer version than the one
//! installed.
//!
//! The rules are libalpm's `alpm_pkg_vercmp`, which pacman and every AUR helper share: the epoch
//! first, then the version, then the release when both have one. Versions are compared segment by
//! segment, where a segment is a run of digits or a run of letters and anything else separates.

use std::cmp::Ordering;

/// How version `left` orders against version `right`, as pacman orders them.
#[must_use]
pub fn vercmp(left: &str, right: &str) -> Ordering {
    if left == right {
        return Ordering::Equal;
    }
    let (left_epoch, left_version, left_release) = split(left);
    let (right_epoch, right_version, right_release) = split(right);
    segments(left_epoch, right_epoch).then_with(|| segments(left_version, right_version)).then_with(|| {
        match (left_release, right_release) {
            (Some(left), Some(right)) => segments(left, right),
            _ => Ordering::Equal,
        }
    })
}

/// Splits `epoch:version-release`; a missing epoch is `0`, a missing release is `None`.
fn split(full: &str) -> (&str, &str, Option<&str>) {
    let digits = full.bytes().take_while(u8::is_ascii_digit).count();
    let (epoch, rest) = match full.as_bytes().get(digits) {
        Some(b':') if digits > 0 => (&full[..digits], &full[digits + 1..]),
        Some(b':') => ("0", &full[1..]),
        _ => ("0", full),
    };
    match rest.rsplit_once('-') {
        Some((version, release)) => (epoch, version, Some(release)),
        None => (epoch, rest, None),
    }
}

/// libalpm's `rpmvercmp`: compares two versions segment by segment.
fn segments(left: &str, right: &str) -> Ordering {
    if left == right {
        return Ordering::Equal;
    }
    let (one, two) = (left.as_bytes(), right.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < one.len() && j < two.len() {
        let (start_i, start_j) = (i, j);
        while i < one.len() && !one[i].is_ascii_alphanumeric() {
            i += 1;
        }
        while j < two.len() && !two[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if i == one.len() || j == two.len() {
            break;
        }
        // Separators of different lengths decide on their own: `1..0` is newer than `1.0`.
        if i - start_i != j - start_j {
            return (i - start_i).cmp(&(j - start_j));
        }
        let numeric = one[i].is_ascii_digit();
        let same_kind = |byte: &u8| if numeric { byte.is_ascii_digit() } else { byte.is_ascii_alphabetic() };
        let end_i = i + one[i..].iter().take_while(|byte| same_kind(byte)).count();
        let end_j = j + two[j..].iter().take_while(|byte| same_kind(byte)).count();
        if end_j == j {
            // A number against letters: the number is newer.
            return if numeric { Ordering::Greater } else { Ordering::Less };
        }
        let (mut a, mut b) = (&one[i..end_i], &two[j..end_j]);
        if numeric {
            a = trim_zeros(a);
            b = trim_zeros(b);
            let longer = a.len().cmp(&b.len());
            if longer != Ordering::Equal {
                return longer;
            }
        }
        let order = a.cmp(b);
        if order != Ordering::Equal {
            return order;
        }
        (i, j) = (end_i, end_j);
    }
    let (left_done, right_done) = (i >= one.len(), j >= two.len());
    if left_done && right_done {
        return Ordering::Equal;
    }
    // One ran out first. An extra letter segment (`1.0a`) is older, an extra number is newer.
    let right_letter = two.get(j).is_some_and(u8::is_ascii_alphabetic);
    let left_letter = one.get(i).is_some_and(u8::is_ascii_alphabetic);
    if (left_done && !right_letter) || left_letter { Ordering::Less } else { Ordering::Greater }
}

/// `digits` without its leading zeros.
fn trim_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|digit| **digit == b'0').count();
    &digits[zeros..]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pairs and what pacman's own `vercmp` program answered for them.
    const RECORDED: [(&str, &str, i8); 30] = [
        ("1.0", "1.0", 0),
        ("1.0", "2.0", -1),
        ("2.0", "1.0", 1),
        ("1.0-1", "1.0-2", -1),
        ("1.0", "1.0-2", 0),
        ("1:1.0", "2.0", 1),
        ("1.0a", "1.0", -1),
        ("1.0", "1.0a", 1),
        ("1.0alpha", "1.0", -1),
        ("1.0.1", "1.0", 1),
        ("1.0", "1.0.1", -1),
        ("1.001", "1.1", 0),
        ("1..0", "1.0", 1),
        ("1.0", "1_0", 0),
        ("1.0a", "1.0b", -1),
        ("a", "1", -1),
        ("1.0rc1", "1.0", -1),
        ("2.12-1", "2.12-1", 0),
        ("0:1.0", "1.0", 0),
        ("1.0.a", "1.0.1", -1),
        ("1.0+2", "1.0.2", 0),
        ("140.0.7339.127-1", "140.0.7339.185-1", -1),
        ("1:1.95.101-1", "1:1.95.102-1", -1),
        ("r1234.abc-1", "r1235.abc-1", -1),
        ("1.0~rc1", "1.0", 1),
        ("5.3.15-1", "5.3.15-1.1", -1),
        ("1.0-1", "1.0", 0),
        ("abc", "abd", -1),
        ("1.2.3", "1.2.3.0", -1),
        ("01", "1", 0),
    ];

    #[test]
    fn orders_as_pacman_does() {
        for (left, right, answer) in RECORDED {
            let expected = answer.cmp(&0);
            assert_eq!(vercmp(left, right), expected, "{left} against {right}");
            assert_eq!(vercmp(right, left), expected.reverse(), "{right} against {left}");
        }
    }

    #[test]
    fn odd_input_never_panics() {
        for text in ["", ":", "-", "::", "1:", ":1", "1-", "-1", "é", "1.é", "999999999999999999999999"] {
            for other in ["", "1", "a", text] {
                let _ = vercmp(text, other);
            }
        }
    }
}

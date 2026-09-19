//! The store's catalog: what can be installed, from where, and how to find it.
//!
//! Like the rest of the core this only reads text and returns structures. Downloading an AUR
//! answer or a popularity list is the application's job, through its runner and the argument
//! list [`net::curl_args`] gives; reading a compressed AppStream file from disk is the
//! application's job too. Every parser here takes what was read and never panics on it: a broken
//! record is skipped and reported, and the rest survives.

pub mod appstream;
pub mod aur;
pub mod category;
pub mod featured;
pub mod flatpak;
pub mod gzip;
mod json;
pub mod merge;
pub mod net;
pub mod popularity;
pub mod repo;
pub mod search;

use std::fmt;

/// Something wrong with one place in a file, reported instead of failing the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The line, counted from 1.
    pub line: usize,
    /// The column in characters, counted from 1.
    pub column: usize,
    /// What is wrong, in English: these reach logs, not the screen.
    pub message: String,
}

impl Problem {
    /// A problem at byte `offset` of `text`. An offset past the end points just after the last
    /// character, so a file cut short still gets a position.
    #[must_use]
    pub fn at(text: &str, offset: usize, message: impl Into<String>) -> Self {
        let mut end = offset.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let before = &text[..end];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        let column = before[line_start..].chars().count() + 1;
        Self { line, column, message: message.into() }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}: {}", self.line, self.column, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_counts_lines_and_characters_from_one() {
        let text = "first\nsecond çğ line\n";
        let problem = Problem::at(text, text.find("line").expect("the word is there"), "here");
        assert_eq!((problem.line, problem.column), (2, 11), "ç and ğ are one column each, not two bytes");
        assert_eq!(problem.to_string(), "2:11: here");
    }

    #[test]
    fn an_offset_past_the_end_or_inside_a_character_still_has_a_position() {
        let text = "ab\nç";
        assert_eq!(Problem::at(text, 99, "cut").line, 2);
        let inside = text.len() - 1;
        let problem = Problem::at(text, inside, "mid");
        assert_eq!((problem.line, problem.column), (2, 1));
    }
}

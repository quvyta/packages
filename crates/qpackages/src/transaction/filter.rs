//! What is read out of pacman's terminal output before it goes into the log view.
//!
//! On a pseudo-terminal pacman and sudo write for a terminal: sudo sends an OSC sequence for
//! terminal integration, pacman hides the cursor and colours its progress bars. The log view
//! draws plain text, so every escape sequence is dropped here; what is kept is the words. The
//! step counter pacman prints in front of each action is read separately to move the progress
//! bar.

/// The escape character that starts every sequence.
const ESC: char = '\u{1b}';

/// The bell that ends an OSC sequence in the old style.
const BEL: char = '\u{7}';

/// `line` without escape sequences and control characters.
///
/// OSC sequences (`ESC ]` up to a bell or `ESC \`) and CSI sequences (`ESC [`, parameters, one
/// final byte) are removed whole, including colour, because the log view shows text and would
/// otherwise draw the sequence's bytes as if they were words. Any other two-character escape is
/// removed too. Control characters other than the tab are dropped; a `\r` never gets here, the
/// process reader already lets it overwrite the line.
#[must_use]
pub fn clean(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ESC {
            match chars.next() {
                Some(']') => skip_osc(&mut chars),
                Some('[') => skip_csi(&mut chars),
                // A two-character escape such as `ESC 7` or `ESC =`; the character went with it.
                Some(_) | None => {}
            }
        } else if c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Skips to the end of an OSC sequence: a bell or `ESC \`. An unterminated one eats the rest of
/// the line, which is what a terminal does with it too.
fn skip_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == BEL {
            return;
        }
        if c == ESC && chars.peek() == Some(&'\\') {
            chars.next();
            return;
        }
    }
}

/// Skips parameter and intermediate bytes up to the final byte of a CSI sequence.
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            return;
        }
    }
}

/// The step pacman is on when `line` starts with its `(k/n)` counter, as a fraction from 0 to 1.
///
/// pacman counts every phase on its own: checking keys, checking integrity, loading files,
/// checking conflicts, then installing. The bar therefore fills once per phase, like pacman's
/// own bars do. The words after the counter are in the user's language and are not read.
#[must_use]
pub fn step(line: &str) -> Option<f32> {
    let rest = line.trim_start().strip_prefix('(')?;
    let (counter, _) = rest.split_once(')')?;
    let (done, total) = counter.split_once('/')?;
    let done: u32 = done.trim().parse().ok()?;
    let total: u32 = total.trim().parse().ok()?;
    // Pacman's counters are small; the conversion is exact.
    (total > 0 && done <= total).then(|| done as f32 / total as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudos_terminal_integration_sequence_is_removed() {
        let line = "\u{1b}]3008;start=2026-09-17T10:15:00Z\u{7}[sudo] password for alice:";
        assert_eq!(clean(line), "[sudo] password for alice:");
        let with_st = "\u{1b}]3008;done\u{1b}\\pacman";
        assert_eq!(clean(with_st), "pacman");
    }

    #[test]
    fn cursor_control_and_colour_sequences_go_and_the_words_stay() {
        assert_eq!(clean("\u{1b}[?25l:: Retrieving packages..."), ":: Retrieving packages...");
        assert_eq!(clean("\u{1b}[1;34m::\u{1b}[0m Synchronizing"), ":: Synchronizing");
        assert_eq!(clean("\u{1b}[2K\u{1b}[1A\u{1b}[?25h gimp  24.9 MiB"), " gimp  24.9 MiB");
        assert_eq!(clean("\u{1b}7plain\u{1b}8"), "plain");
    }

    #[test]
    fn an_unterminated_sequence_does_not_panic() {
        assert_eq!(clean("\u{1b}]3008;forever"), "");
        assert_eq!(clean("\u{1b}[1;2"), "");
        assert_eq!(clean("\u{1b}"), "");
    }

    #[test]
    fn control_characters_other_than_the_tab_are_dropped() {
        assert_eq!(clean("a\u{7}b\tc"), "ab\tc");
        assert_eq!(clean("Türkçe ✓"), "Türkçe ✓");
    }

    #[test]
    fn the_step_counter_becomes_a_fraction() {
        assert_eq!(step("(1/9) checking keys in keyring"), Some(1.0 / 9.0));
        assert_eq!(step("( 3/12) installing babl"), Some(0.25));
        assert_eq!(step("(12/12) kuruluyor gimp"), Some(1.0));
        assert_eq!(step(":: Retrieving packages..."), None);
        assert_eq!(step("(0/0) nothing"), None);
        assert_eq!(step("(5/3) impossible"), None);
        assert_eq!(step("(a/b)"), None);
    }
}

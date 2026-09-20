//! What a background update check leaves for the screen: the state file.
//!
//! `qpac --check` runs without privileges, from a user timer. It writes the updates it found and
//! when it found them to `$XDG_STATE_HOME/quvyta/packages/state.json`; the screen reads the file
//! when it opens, and runs its own check when there is none. This module holds the file's format,
//! the rule that keeps checks from hammering the mirrors, and the comparison that tells an AUR
//! update from the versions installed.

use std::collections::HashMap;

use tinyjson::JsonValue;

use crate::catalog::Problem;
use crate::catalog::aur::AurPackage;
use crate::pacman::{Update, vercmp};

/// The format of the state file, written into it so a later release can tell an older file.
pub const STATE_VERSION: i64 = 1;

/// The shortest time between two successful checks, in seconds: an hour, however often the
/// check is started. A login loop or a timer set by hand never makes qpac hit the mirrors more.
pub const MIN_INTERVAL: i64 = 3600;

/// How deeply the state file may nest; it nests three levels, and the limit keeps a damaged file
/// from exhausting the stack of the recursive parser.
const MAX_DEPTH: usize = 8;

/// What the last successful check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateState {
    /// When it finished, in seconds since the Unix epoch.
    pub checked: i64,
    /// The updates waiting in the repositories.
    pub pacman: Vec<Update>,
    /// The updates waiting in the AUR.
    pub aur: Vec<Update>,
}

impl UpdateState {
    /// The file's text: a JSON object, the same for the same state.
    #[must_use]
    pub fn to_json(&self) -> String {
        let list = |updates: &[Update]| {
            let items: Vec<String> = updates
                .iter()
                .map(|update| {
                    format!(
                        "{{\"name\":{},\"from\":{},\"to\":{},\"ignored\":{}}}",
                        quoted(&update.name),
                        quoted(&update.from),
                        quoted(&update.to),
                        update.ignored
                    )
                })
                .collect();
            format!("[{}]", items.join(","))
        };
        format!(
            "{{\"version\":{STATE_VERSION},\"checked\":{},\"pacman\":{},\"aur\":{}}}\n",
            self.checked,
            list(&self.pacman),
            list(&self.aur)
        )
    }

    /// Reads the file's text.
    ///
    /// # Errors
    ///
    /// Returns where the text breaks, or which field is missing or of another format version.
    pub fn parse(text: &str) -> Result<Self, Problem> {
        let at_start = |message: &str| Problem::at(text, 0, message);
        if let Some(offset) = too_deep(text) {
            return Err(Problem::at(text, offset, format!("nested more than {MAX_DEPTH} levels deep")));
        }
        let value: JsonValue = text.parse().map_err(|error: tinyjson::JsonParseError| Problem {
            line: error.line(),
            column: error.column(),
            message: error.to_string(),
        })?;
        let object = value.get::<HashMap<String, JsonValue>>().ok_or_else(|| at_start("not an object"))?;
        let number = |key: &str| object.get(key).and_then(|value| value.get::<f64>()).copied();
        if number("version").is_none_or(|version| version != 1.0) {
            return Err(at_start("an unknown format version"));
        }
        let checked = number("checked")
            .filter(|seconds| seconds.fract() == 0.0 && seconds.abs() < 9.0e15)
            .ok_or_else(|| at_start("no time of the last check"))?;
        let list = |key: &str| -> Result<Vec<Update>, Problem> {
            let items = object
                .get(key)
                .and_then(|value| value.get::<Vec<JsonValue>>())
                .ok_or_else(|| at_start(&format!("no `{key}` list")))?;
            items
                .iter()
                .map(|item| read_update(item).ok_or_else(|| at_start(&format!("a broken `{key}` entry"))))
                .collect()
        };
        // `checked` passed the range test above, so the conversion is exact.
        #[expect(clippy::cast_possible_truncation, reason = "a whole number below 2^53")]
        let checked = checked as i64;
        Ok(Self { checked, pacman: list("pacman")?, aur: list("aur")? })
    }
}

/// Whether a check that succeeded at `last` is too recent for another at `now`: less than
/// [`MIN_INTERVAL`] ago. A last check dated after `now` means the clock moved back, and does
/// not hold the next one off.
#[must_use]
pub fn checked_recently(last: Option<i64>, now: i64) -> bool {
    last.is_some_and(|last| last <= now && now - last < MIN_INTERVAL)
}

/// The AUR updates among `foreign`, the installed packages no repository knows (`name`,
/// `version`), given what the AUR answered for them: every package the AUR holds in a newer
/// version, by name.
#[must_use]
pub fn aur_updates(foreign: &[(String, String)], aur: &[AurPackage]) -> Vec<Update> {
    let mut updates: Vec<Update> = foreign
        .iter()
        .filter_map(|(name, installed)| {
            let package = aur.iter().find(|package| package.name == *name)?;
            vercmp(&package.version, installed).is_gt().then(|| Update {
                name: name.clone(),
                from: installed.clone(),
                to: package.version.clone(),
                ignored: false,
            })
        })
        .collect();
    updates.sort_by(|left, right| left.name.cmp(&right.name));
    updates
}

fn read_update(value: &JsonValue) -> Option<Update> {
    let object = value.get::<HashMap<String, JsonValue>>()?;
    let text = |key: &str| object.get(key)?.get::<String>().cloned();
    Some(Update {
        name: text("name")?,
        from: text("from")?,
        to: text("to")?,
        ignored: object.get("ignored").and_then(|value| value.get::<bool>()).copied().unwrap_or(false),
    })
}

/// `text` as a JSON string.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The byte offset where nesting first passes [`MAX_DEPTH`], ignoring brackets inside strings.
fn too_deep(text: &str) -> Option<usize> {
    let (mut depth, mut in_string, mut escaped) = (0_usize, false, false);
    for (offset, byte) in text.bytes().enumerate() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'[' | b'{' if !in_string => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Some(offset);
                }
            }
            b']' | b'}' if !in_string => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(name: &str, from: &str, to: &str) -> Update {
        Update { name: name.to_owned(), from: from.to_owned(), to: to.to_owned(), ignored: false }
    }

    fn state() -> UpdateState {
        UpdateState {
            checked: 1_784_937_600,
            pacman: vec![
                update("bash", "5.3.15-1", "5.3.16-1"),
                Update { ignored: true, ..update("linux", "6.18.1-1", "6.18.2-1") },
            ],
            aur: vec![update("brave-bin", "1:1.95.101-1", "1:1.95.102-1")],
        }
    }

    #[test]
    fn the_state_reads_back_as_it_was_written() {
        let text = state().to_json();
        assert!(text.starts_with("{\"version\":1,\"checked\":1784937600,\"pacman\":[{\"name\":\"bash\""));
        assert_eq!(UpdateState::parse(&text), Ok(state()));
        let empty = UpdateState { checked: 0, pacman: Vec::new(), aur: Vec::new() };
        assert_eq!(UpdateState::parse(&empty.to_json()), Ok(empty));
    }

    #[test]
    fn odd_characters_survive_the_round_trip() {
        let odd = UpdateState { checked: 1, pacman: vec![update("a\"b\\c", "1\n2", "ç\u{1}")], aur: Vec::new() };
        assert_eq!(UpdateState::parse(&odd.to_json()), Ok(odd));
    }

    #[test]
    fn a_damaged_file_says_where_and_never_panics() {
        let problem = UpdateState::parse("{\"version\":1,\n\"checked\":}").expect_err("broken");
        assert_eq!(problem.line, 2);
        for text in [
            "",
            "[]",
            "{}",
            "{\"version\":2,\"checked\":1,\"pacman\":[],\"aur\":[]}",
            "{\"version\":1,\"checked\":1.5,\"pacman\":[],\"aur\":[]}",
            "{\"version\":1,\"checked\":1,\"pacman\":[{}],\"aur\":[]}",
            "{\"version\":1,\"checked\":1,\"pacman\":[]}",
            &"[".repeat(100_000),
        ] {
            assert!(UpdateState::parse(text).is_err(), "`{}`", text.chars().take(40).collect::<String>());
        }
        let whole = state().to_json();
        for cut in 0..whole.len() {
            let _ = UpdateState::parse(&whole[..cut]);
        }
    }

    #[test]
    fn a_check_within_the_hour_is_too_recent() {
        let now = 1_784_937_600;
        assert!(!checked_recently(None, now));
        assert!(checked_recently(Some(now), now));
        assert!(checked_recently(Some(now - 3599), now));
        assert!(!checked_recently(Some(now - 3600), now));
        assert!(!checked_recently(Some(now + 60), now), "a clock that moved back does not hold checks off");
    }

    #[test]
    fn only_newer_aur_versions_are_updates() {
        let foreign = [
            ("paru".to_owned(), "2.1.0-1".to_owned()),
            ("brave-bin".to_owned(), "1:1.95.101-1".to_owned()),
            ("hand-built".to_owned(), "1.0-1".to_owned()),
            ("yay".to_owned(), "12.5.0-1".to_owned()),
        ];
        let aur = |name: &str, version: &str| AurPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            ..AurPackage::default()
        };
        let answered = [aur("paru", "2.1.0-1"), aur("brave-bin", "1:1.95.102-1"), aur("yay", "12.4.2-1")];
        assert_eq!(aur_updates(&foreign, &answered), [update("brave-bin", "1:1.95.101-1", "1:1.95.102-1")]);
    }
}

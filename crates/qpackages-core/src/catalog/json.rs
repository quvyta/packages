//! Reading the JSON answers of the AUR, pkgstats and Flathub.
//!
//! Answers are read into a generic value and picked apart field by field, so a field a server
//! adds or drops, or sends with another type, never fails the whole answer: the field alone
//! takes its empty value.

use std::collections::HashMap;

use tinyjson::JsonValue;

use super::Problem;

/// How deeply arrays and objects may nest. The answers read here nest three levels; the limit
/// keeps a hostile answer of a million `[` from exhausting the stack of a recursive parser.
const MAX_DEPTH: usize = 64;

/// Parses a whole answer, reporting where it breaks.
pub(crate) fn parse(text: &str) -> Result<JsonValue, Problem> {
    if let Some(offset) = too_deep(text) {
        return Err(Problem::at(text, offset, format!("nested more than {MAX_DEPTH} levels deep")));
    }
    text.parse().map_err(|error: tinyjson::JsonParseError| Problem {
        line: error.line(),
        column: error.column(),
        message: error.to_string(),
    })
}

/// The byte offset where nesting first passes [`MAX_DEPTH`], ignoring brackets inside strings.
fn too_deep(text: &str) -> Option<usize> {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
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

/// The value under `key` when `value` is an object that has it.
pub(crate) fn field<'a>(value: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
    value.get::<HashMap<String, JsonValue>>()?.get(key)
}

/// The string under `key`.
pub(crate) fn text<'a>(value: &'a JsonValue, key: &str) -> Option<&'a str> {
    field(value, key)?.get::<String>().map(String::as_str)
}

/// The array under `key`.
pub(crate) fn array<'a>(value: &'a JsonValue, key: &str) -> Option<&'a Vec<JsonValue>> {
    field(value, key)?.get::<Vec<JsonValue>>()
}

/// The strings of the array under `key`; entries of another type are left out.
pub(crate) fn strings(value: &JsonValue, key: &str) -> Vec<String> {
    array(value, key)
        .map(|items| items.iter().filter_map(|item| item.get::<String>().cloned()).collect())
        .unwrap_or_default()
}

/// The number under `key`.
pub(crate) fn number(value: &JsonValue, key: &str) -> Option<f64> {
    field(value, key)?.get::<f64>().copied()
}

/// The whole number under `key`. JSON numbers are doubles, so anything with a fraction or past
/// 2^53, where doubles stop counting exactly, is not one.
pub(crate) fn integer(value: &JsonValue, key: &str) -> Option<i64> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    let number = number(value, key)?;
    // Checked first, so the conversion is exact.
    (number.fract() == 0.0 && number.abs() <= EXACT).then_some(number as i64)
}

/// The count under `key`: a whole number that is not negative.
pub(crate) fn count(value: &JsonValue, key: &str) -> Option<u64> {
    integer(value, key).and_then(|number| u64::try_from(number).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_of_the_wrong_type_are_absent() {
        let value =
            parse(r#"{"a": "x", "n": 3, "f": 1.5, "big": 1e300, "neg": -2, "list": ["p", 4, "q"]}"#).expect("valid");
        assert_eq!(text(&value, "a"), Some("x"));
        assert_eq!(text(&value, "n"), None);
        assert_eq!(integer(&value, "n"), Some(3));
        assert_eq!(integer(&value, "f"), None, "a fraction is not a whole number");
        assert_eq!(integer(&value, "big"), None, "past 2^53 a double does not count exactly");
        assert_eq!(count(&value, "neg"), None);
        assert_eq!(number(&value, "f"), Some(1.5));
        assert_eq!(strings(&value, "list"), ["p", "q"]);
        assert!(strings(&value, "missing").is_empty());
        assert_eq!(field(&parse("[1]").expect("valid"), "a"), None, "an array has no fields");
    }

    #[test]
    fn a_broken_answer_says_where() {
        let problem = parse("{\n  \"a\": }").expect_err("broken");
        assert_eq!(problem.line, 2);
    }

    #[test]
    fn deep_nesting_is_refused_before_parsing() {
        let deep = "[".repeat(100_000);
        let problem = parse(&deep).expect_err("far too deep");
        assert_eq!(problem.column, MAX_DEPTH + 1);
        let fine = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&fine).is_ok());
        assert!(
            parse(r#"{"s": "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[\"["}"#).is_ok(),
            "brackets in strings do not count"
        );
    }
}

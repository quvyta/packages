//! Every language file against the English one: the same keys, the same placeholders, the plural
//! forms its language needs, and a system setting that finds it. Whether the embedded fonts can
//! draw every character is asked where the pictures are drawn, in `tests/readme_shots.rs`, since
//! only the tool that draws them knows what its fonts hold.
//!
//! The checks read the files themselves rather than the loaded catalogue, so a key a language
//! quietly borrows from English on screen still counts as missing here.

use std::collections::{BTreeMap, BTreeSet};

use qframe::i18n::PluralCategory;
use toml::de::{DeTable, DeValue};

use super::{LOCALES, env};

/// One message of a file: a plain text, or its plural forms by category name.
#[derive(Debug)]
enum Entry {
    Plain(String),
    Plural(BTreeMap<String, String>),
}

/// A parsed language file: its file name, its code and every message under its dotted key.
struct File {
    name: &'static str,
    code: String,
    entries: BTreeMap<String, Entry>,
}

fn parse(name: &'static str, text: &str) -> File {
    let root = DeTable::parse(text).unwrap_or_else(|error| panic!("{name} is not TOML: {error}"));
    let mut code = String::new();
    let mut entries = BTreeMap::new();
    for (section, value) in root.get_ref() {
        let DeValue::Table(table) = value.get_ref() else { panic!("{name}: `{}` is not a section", section.get_ref()) };
        if section.get_ref() == "meta" {
            if let Some(DeValue::String(value)) = table.get("code").map(toml::Spanned::get_ref) {
                code = value.to_string();
            }
            continue;
        }
        flatten(name, section.get_ref(), table, &mut entries);
    }
    assert!(!code.is_empty(), "{name} has no meta.code");
    File { name, code, entries }
}

/// Whether a table is a plural message rather than a section: all its keys are plural category
/// names.
fn is_plural(table: &DeTable<'_>) -> bool {
    !table.is_empty() && table.keys().all(|key| PluralCategory::from_name(key.get_ref()).is_some())
}

fn flatten(name: &str, prefix: &str, table: &DeTable<'_>, out: &mut BTreeMap<String, Entry>) {
    for (key, value) in table {
        let full = format!("{prefix}.{}", key.get_ref());
        match value.get_ref() {
            DeValue::String(text) => {
                out.insert(full, Entry::Plain(text.to_string()));
            }
            DeValue::Table(inner) if is_plural(inner) => {
                let forms = inner
                    .iter()
                    .map(|(category, text)| match text.get_ref() {
                        DeValue::String(text) => (category.get_ref().to_string(), text.to_string()),
                        _ => panic!("{name}: `{full}.{}` is not text", category.get_ref()),
                    })
                    .collect();
                out.insert(full, Entry::Plural(forms));
            }
            DeValue::Table(inner) => flatten(name, &full, inner, out),
            _ => panic!("{name}: `{full}` is neither text nor a table"),
        }
    }
}

fn files() -> Vec<File> {
    LOCALES.iter().map(|(name, text)| parse(name, text)).collect()
}

fn english() -> File {
    files().into_iter().find(|file| file.code == "en").expect("the English file is compiled in")
}

/// The `{name}` placeholders of a text.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        found.insert(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    found
}

/// Every text a message holds: the one plain text, or one per plural form.
fn texts_of(entry: &Entry) -> Vec<&String> {
    match entry {
        Entry::Plain(text) => vec![text],
        Entry::Plural(forms) => forms.values().collect(),
    }
}

/// The plural categories whole counts fall into in a language, by the framework's own rule, so a
/// file is asked for exactly the forms the screen can pick.
fn categories_for(code: &str) -> BTreeSet<String> {
    // The framework reads the language out of a regional or script code itself, so `pt-BR` is
    // asked for Brazilian Portuguese's forms rather than a guess made from `pt`.
    let mut needed: BTreeSet<String> = (0..=200).map(|n| PluralCategory::of(code, n).name().to_owned()).collect();
    // `other` is what every lookup falls back to, so it is always written.
    needed.insert("other".to_owned());
    needed
}

/// The codes of the compiled-in languages.
pub(crate) fn codes() -> Vec<String> {
    files().into_iter().map(|file| file.code).collect()
}

/// The plain text `key` holds in the language `code`, read from the language file itself.
///
/// A screen test asserts this text is on screen whole, so it must be a fixed label: a plural or a
/// text with a `{placeholder}` is not one, and asking for one is a mistake in the test.
pub(crate) fn label(code: &str, key: &str) -> String {
    let file = files().into_iter().find(|file| file.code == code).unwrap_or_else(|| panic!("`{code}` is compiled in"));
    let entry = file.entries.get(key).unwrap_or_else(|| panic!("`{key}` is not in {}", file.name));
    let Entry::Plain(text) = entry else { panic!("`{key}` in {} is a plural, not a fixed label", file.name) };
    assert!(placeholders(text).is_empty(), "`{key}` in {} carries a placeholder, so it is not fixed", file.name);
    text.clone()
}

/// A system's `LANG` for each language qpac may carry, with the awkward ones written out: a
/// region (`pt_BR`), a country that stands for a script (`zh_CN` is Simplified Chinese), and a
/// country with no file of its own (`pt_PT`).
const SYSTEM: &[(&str, &str)] = &[
    ("en_GB.UTF-8", "en"),
    ("tr_TR.UTF-8", "tr"),
    ("de_AT.UTF-8", "de"),
    ("es_MX.UTF-8", "es"),
    ("fr_CA.UTF-8", "fr"),
    ("pt_BR.UTF-8", "pt-BR"),
    ("pt_PT.UTF-8", "pt-BR"),
    ("ru_RU.UTF-8", "ru"),
    ("zh_CN.UTF-8", "zh-Hans"),
    ("zh_SG.UTF-8", "zh-Hans"),
    ("ja_JP.UTF-8", "ja"),
];

#[test]
fn every_file_loads_cleanly_and_names_its_language() {
    let env = env();
    assert_eq!(env.diagnostics(), &[], "a language file has a problem");
    assert_eq!(LOCALES[0].0, "en.toml", "English is the reference and comes first");
    let listed = env.i18n().list();
    for file in files() {
        assert_eq!(format!("{}.toml", file.code), file.name, "the code in [meta] names the file");
        let name = listed.iter().find(|(code, _)| *code == file.code).map(|(_, name)| name.clone());
        assert!(name.is_some_and(|name| !name.trim().is_empty()), "`{}` has no name to show", file.code);
    }
    let names: BTreeSet<&str> = listed.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(names.len(), listed.len(), "two languages share a name: {listed:?}");
}

#[test]
fn every_language_carries_every_english_key_and_nothing_else() {
    let english = english();
    let env = env();
    for file in files() {
        let missing: Vec<&String> = english.entries.keys().filter(|key| !file.entries.contains_key(*key)).collect();
        assert!(missing.is_empty(), "`{}` lacks {missing:?}", file.code);
        let extra: Vec<&String> = file.entries.keys().filter(|key| !english.entries.contains_key(*key)).collect();
        assert!(extra.is_empty(), "`{}` has keys English does not: {extra:?}", file.code);
        // The loaded catalogue agrees with the file: each key is the language's own text.
        for key in english.entries.keys() {
            assert!(env.i18n().has(&file.code, key), "`{key}` has no text of its own in `{}`", file.code);
        }
    }
}

#[test]
fn every_translation_keeps_the_english_placeholders() {
    let english = english();
    for file in files() {
        for (key, entry) in &file.entries {
            let Some(source) = english.entries.get(key) else { continue };
            for text in texts_of(entry) {
                assert!(!text.trim().is_empty(), "`{key}` is empty in `{}`", file.code);
            }
            match (source, entry) {
                (Entry::Plain(source), Entry::Plain(text)) => {
                    let (wanted, written) = (placeholders(source), placeholders(text));
                    assert_eq!(written, wanted, "`{key}` in `{}` changes the placeholders: {text}", file.code);
                }
                (Entry::Plural(_), Entry::Plural(_)) => {
                    // A plural is read form by form only as far as English itself is even: English
                    // writes "hour" beside "{n} hours", and a language may do the same in any of
                    // its forms. What no form may do is name a placeholder English does not have,
                    // which is how a misspelt `{nmae}` reaches the screen, and what the message as
                    // a whole may not do is lose one, which is how a number disappears.
                    let wanted: BTreeSet<String> =
                        texts_of(source).into_iter().flat_map(|text| placeholders(text)).collect();
                    let mut written = BTreeSet::new();
                    for text in texts_of(entry) {
                        let here = placeholders(text);
                        let unknown: Vec<&String> = here.difference(&wanted).collect();
                        assert!(unknown.is_empty(), "`{key}` in `{}` invents {unknown:?}: {text}", file.code);
                        written.extend(here);
                    }
                    assert_eq!(written, wanted, "`{key}` in `{}` loses a placeholder", file.code);
                }
                _ => panic!("`{key}` is a plural in one file and plain text in the other"),
            }
        }
    }
}

#[test]
fn every_plural_has_exactly_the_forms_its_language_uses() {
    for file in files() {
        let needed = categories_for(&file.code);
        for (key, entry) in &file.entries {
            if let Entry::Plural(forms) = entry {
                let written: BTreeSet<String> = forms.keys().cloned().collect();
                assert_eq!(written, needed, "`{key}` in `{}` has the wrong plural forms", file.code);
            }
        }
    }
}

#[test]
fn the_categories_follow_each_language() {
    let set = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect::<BTreeSet<String>>();
    assert_eq!(categories_for("en"), set(&["one", "other"]));
    assert_eq!(categories_for("tr"), set(&["one", "other"]));
    assert_eq!(categories_for("de"), set(&["one", "other"]));
    // Spanish, French and Portuguese keep `many` for millions, which no count on these screens
    // reaches, so a whole number falls into `one` or `other` there.
    assert_eq!(categories_for("es"), set(&["one", "other"]));
    assert_eq!(categories_for("fr"), set(&["one", "other"]));
    assert_eq!(categories_for("pt-BR"), set(&["one", "other"]));
    assert_eq!(categories_for("ru"), set(&["one", "few", "many", "other"]));
    assert_eq!(categories_for("zh-Hans"), set(&["other"]));
    assert_eq!(categories_for("ja"), set(&["other"]));
}

#[test]
fn the_system_language_picks_the_matching_file() {
    let env = env();
    let here = codes();
    let detect = |lang: &str| {
        let lang = lang.to_owned();
        env.i18n().detect(move |name| (name == "LANG").then(|| lang.clone()))
    };
    for (system, wanted) in SYSTEM {
        if here.iter().any(|code| code == wanted) {
            assert_eq!(detect(system).as_deref(), Some(*wanted), "{system}");
        }
    }
    // Every language compiled in is named here, so adding one to `LOCALES` without saying which
    // system setting finds it does not pass unnoticed.
    for code in &here {
        assert!(SYSTEM.iter().any(|(_, wanted)| wanted == code), "no system setting is listed for `{code}`");
    }
    assert_eq!(detect("C.UTF-8"), None, "C names no language");
}

#[test]
fn the_framework_speaks_every_language_the_application_does() {
    // Dialog buttons, key names and pickers come from the framework; a gap there would put
    // English words on an otherwise translated screen.
    let env = env();
    for file in files() {
        let missing = env.i18n().missing_keys(&file.code, "en");
        assert!(missing.is_empty(), "`{}` falls back to English for {missing:?}", file.code);
    }
}

#[test]
fn every_language_names_the_file_the_helper_really_writes() {
    // The sentence under the mirror settings tells the user where the replaced list is kept, and
    // it spells the name out in all nine files. Rename the constant alone and every one of them
    // would send the user looking for a file that is not there.
    let kept = qpackages_core::reflector::BACKUP;
    for file in files() {
        let Some(Entry::Plain(text)) = file.entries.get("mirrors.apply-keeps") else {
            panic!("`mirrors.apply-keeps` is missing from {}", file.name)
        };
        assert!(text.contains(kept), "{} does not name `{kept}`: {text}", file.name);
    }
}

#[test]
fn the_checks_catch_what_they_are_for() {
    // A check nobody has tested is not a check: each rule above is shown refusing the mistake it
    // is there for, on made-up files that are never compiled in.
    let english = parse(
        "en.toml",
        "[meta]\nname = \"English\"\ncode = \"en\"\n[a]\nhello = \"Hi {name}\"\nn = { one = \"{n} file\", other = \"{n} files\" }\n",
    );
    let russian = parse(
        "ru.toml",
        "[meta]\nname = \"Русский\"\ncode = \"ru\"\n[a]\nhello = \"Привет {nmae}\"\nn = { one = \"{n} файл\", other = \"{n} файла\" }\n",
    );
    assert_eq!(russian.code, "ru");

    // A key English has and the translation does not, and one only the translation has.
    let short = parse("de.toml", "[meta]\nname = \"Deutsch\"\ncode = \"de\"\n[a]\nhello = \"Hallo {name}\"\n");
    assert!(english.entries.keys().any(|key| !short.entries.contains_key(key)), "a missing key is caught");
    let odd = parse("es.toml", "[meta]\nname = \"Español\"\ncode = \"es\"\n[a]\nbye = \"Adiós\"\n");
    assert!(odd.entries.keys().any(|key| !english.entries.contains_key(key)), "a key English lacks is caught");

    // A placeholder misspelt: the text still reads, but the name never arrives.
    let Entry::Plain(source) = &english.entries["a.hello"] else { panic!("plain text reads as plain text") };
    let Entry::Plain(target) = &russian.entries["a.hello"] else { panic!("plain text reads as plain text") };
    assert_ne!(placeholders(target), placeholders(source), "a misspelt placeholder is caught");

    // A Russian plural without `few`: three files would silently read as "3 файла" from `other`.
    let Entry::Plural(forms) = &russian.entries["a.n"] else { panic!("a plural reads as a plural") };
    let written: BTreeSet<String> = forms.keys().cloned().collect();
    assert_ne!(written, categories_for("ru"), "a Russian table without `few` is what the plural check refuses");
    assert_eq!(PluralCategory::of("ru", 3).name(), "few");

    // `label` refuses what a narrow-screen check cannot look for whole.
    let with_placeholder = parse("en.toml", "[meta]\nname = \"English\"\ncode = \"en\"\n[a]\nb = \"Hi {name}\"\n");
    let Entry::Plain(text) = &with_placeholder.entries["a.b"] else { panic!("plain text reads as plain text") };
    assert!(!placeholders(text).is_empty(), "`label` refuses a text whose words change with a placeholder");
}

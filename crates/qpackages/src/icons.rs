//! The icon before a package's name.
//!
//! Three places answer, in this order: the glyph table (`assets/icons/packages.toml`), which
//! names a Nerd Font glyph for well-known packages; the icon of the package's kind; and the icon
//! of the source it came from. The table only speaks in Nerd mode, since its glyphs exist only
//! in a Nerd Font; the kind and source icons have a glyph for every mode, so every package has an
//! icon whatever the terminal can draw.
//!
//! The kind and source icons are the application's own icon set (`assets/icons/qpackages.toml`),
//! given to the runtime like its language files. The framework draws icon keys only from the
//! set the theme chooses, so the glyphs are looked up here, through the framework's own icon set
//! reader, in the glyph mode the screen is drawn in.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use qframe::diagnostics::{Diagnostic, Location};
use qframe::icons::{GlyphMode, IconSetRegistry, Icons};
use qpackages_core::catalog::category::Category;
use qpackages_core::pacman::Package;
use qpackages_core::sources::Source;

/// The glyph table, compiled in: an installed program reads nothing beside itself.
const TABLE: (&str, &str) = ("packages.toml", include_str!("../assets/icons/packages.toml"));

/// The kind and source icons, as an icon set file the runtime is given.
pub const SET: (&str, &str) = ("qpackages.toml", include_str!("../assets/icons/qpackages.toml"));

/// The set's id: the stem of its file name, the way the framework names sets.
const SET_ID: &str = "qpackages";

/// The glyph table, read once.
static GLYPHS: LazyLock<(GlyphTable, Vec<Diagnostic>)> = LazyLock::new(|| GlyphTable::parse(TABLE.0, TABLE.1));

/// The kind and source icons in each glyph mode, built once, in the order of [`MODES`].
static SET_ICONS: LazyLock<[Icons; 3]> = LazyLock::new(|| {
    let mut registry = IconSetRegistry::builtin();
    registry.add_source(SET_ID, SET.0, SET.1);
    let none = BTreeMap::new();
    MODES.map(|mode| registry.icons(SET_ID, &none, mode))
});

/// What went wrong reading the glyph table; empty for the table that ships.
pub fn diagnostics() -> &'static [Diagnostic] {
    &GLYPHS.1
}

/// The icon of an installed pacman package in `mode`. Its kind comes from its groups and its name,
/// the hints the local database has.
#[must_use]
pub fn installed(package: &Package, mode: GlyphMode) -> String {
    const NO_CATEGORIES: [&str; 0] = [];
    let kind = Category::classify(&NO_CATEGORIES, &package.groups, &package.name);
    glyph(&package.name, kind, Source::Pacman, mode)
}

/// The icon of the package or Flatpak id `name`, of `kind`, from `source`, in `mode`.
#[must_use]
pub fn glyph(name: &str, kind: Category, source: Source, mode: GlyphMode) -> String {
    if mode == GlyphMode::Nerd
        && let Some(glyph) = GLYPHS.0.lookup(name)
    {
        return glyph.to_string();
    }
    let icons = &SET_ICONS[mode_index(mode)];
    let key = if kind == Category::Unknown {
        format!("source.{}", crate::sources::name(source))
    } else {
        format!("category.{}", kind.key())
    };
    icons.glyph(&key).into_owned()
}

/// Every glyph mode, in the order of [`mode_index`].
pub const MODES: [GlyphMode; 3] = [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii];

/// Where `mode` sits in [`MODES`], for anything kept once per mode.
pub const fn mode_index(mode: GlyphMode) -> usize {
    match mode {
        GlyphMode::Nerd => 0,
        GlyphMode::Unicode => 1,
        GlyphMode::Ascii => 2,
    }
}

/// Package names and name prefixes, each with its Nerd Font glyph.
#[derive(Debug, Default)]
pub struct GlyphTable {
    exact: HashMap<String, char>,
    /// Longest prefix first, so the first match is the most specific one.
    prefixes: Vec<(String, char)>,
}

impl GlyphTable {
    /// Reads the table's text. Every line that is not a `"key" = "code point"` entry under
    /// `[icons]`, a comment or blank becomes a diagnostic naming its file, line and column, and
    /// the rest of the table still loads.
    #[must_use]
    pub fn parse(file: &str, text: &str) -> (Self, Vec<Diagnostic>) {
        let mut table = Self::default();
        let mut problems = Vec::new();
        let mut in_icons = false;
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            let content = line.trim_end_matches(['\n', '\r']);
            let trimmed = content.trim_start();
            let indent = content.len() - trimmed.len();
            let problem = |at: usize, message: String| {
                Diagnostic::error(Some(Location::from_offset(file, text, start + indent + at)), message)
            };
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') {
                in_icons = trimmed.trim_end() == "[icons]";
                if !in_icons {
                    problems.push(problem(0, format!("unknown section `{}`; expected [icons]", trimmed.trim_end())));
                }
                continue;
            }
            if !in_icons {
                problems.push(problem(0, "an entry outside the [icons] section".to_owned()));
                continue;
            }
            let added =
                entry(trimmed).and_then(|(key, glyph)| table.insert(key, glyph).map_err(|message| (0, message)));
            if let Err((at, message)) = added {
                problems.push(problem(at, message));
            }
        }
        table.prefixes.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        (table, problems)
    }

    /// Adds one row; a key given twice keeps its first glyph.
    fn insert(&mut self, key: &str, glyph: char) -> Result<(), String> {
        let added = match key.strip_suffix('*') {
            Some(prefix) if self.prefixes.iter().any(|(known, _)| known == prefix) => false,
            Some(prefix) => {
                self.prefixes.push((prefix.to_owned(), glyph));
                true
            }
            None => match self.exact.entry(key.to_owned()) {
                Entry::Vacant(slot) => {
                    slot.insert(glyph);
                    true
                }
                Entry::Occupied(_) => false,
            },
        };
        if added { Ok(()) } else { Err(format!("`{key}` is listed twice; the first glyph is kept")) }
    }

    /// The glyph for the package or Flatpak id `name`.
    ///
    /// The name is tried as it is, then without an `-bin` or `-git` ending and a `lib32-`
    /// beginning, which name builds of the same program. Any exact row wins over every prefix,
    /// and a longer prefix over a shorter one.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<char> {
        let bare = name.strip_prefix("lib32-").unwrap_or(name);
        let bare = bare.strip_suffix("-bin").or_else(|| bare.strip_suffix("-git")).unwrap_or(bare);
        let names = [name, bare];
        names.iter().find_map(|name| self.exact.get(*name).copied()).or_else(|| {
            names.iter().find_map(|name| {
                self.prefixes.iter().find(|(prefix, _)| name.starts_with(prefix.as_str())).map(|(_, glyph)| *glyph)
            })
        })
    }
}

/// Reads `"key" = "hex"` with an optional `# comment` after it, or says at which byte of the line
/// it stopped making sense.
fn entry(line: &str) -> Result<(&str, char), (usize, String)> {
    let (key, after_key) = quoted(line, 0).ok_or((0, "expected a quoted package name".to_owned()))?;
    if key.is_empty() || key == "*" {
        return Err((0, "an empty package name matches nothing".to_owned()));
    }
    if key[..key.len() - 1].contains('*') {
        return Err((0, format!("`{key}`: `*` may only end a prefix")));
    }
    let equals = skip_spaces(line, after_key);
    if !line[equals..].starts_with('=') {
        return Err((equals, "expected `=` after the package name".to_owned()));
    }
    let value_at = skip_spaces(line, equals + 1);
    let (code, after_value) = quoted(line, value_at).ok_or((value_at, "expected a quoted code point".to_owned()))?;
    let rest = skip_spaces(line, after_value);
    if rest < line.len() && !line[rest..].starts_with('#') {
        return Err((rest, "unexpected text after the code point".to_owned()));
    }
    let glyph = u32::from_str_radix(code, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or((value_at, format!("`{code}` is not a hexadecimal code point")))?;
    if qframe::text::width(&glyph.to_string()) != 1 {
        return Err((value_at, format!("`{code}` is not one cell wide")));
    }
    Ok((key, glyph))
}

/// The text between the double quotes that open at byte `at` of `line`, and the byte after the
/// closing one. The table's names and code points have no escapes, so a backslash is refused.
fn quoted(line: &str, at: usize) -> Option<(&str, usize)> {
    let rest = line[at..].strip_prefix('"')?;
    let end = rest.find('"')?;
    let text = &rest[..end];
    (!text.contains('\\')).then_some((text, at + 1 + end + 1))
}

/// The first byte at or after `at` that is not a space or a tab.
fn skip_spaces(line: &str, at: usize) -> usize {
    line[at..].find(|c| c != ' ' && c != '\t').map_or(line.len(), |found| at + found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: char = '\u{e745}';

    fn table(text: &str) -> GlyphTable {
        let (table, problems) = GlyphTable::parse("test.toml", text);
        assert!(problems.is_empty(), "{problems:?}");
        table
    }

    #[test]
    fn the_shipped_table_reads_without_a_single_problem() {
        assert!(diagnostics().is_empty(), "{:?}", diagnostics());
        assert!(GLYPHS.0.exact.len() + GLYPHS.0.prefixes.len() > 100, "the whole table was read");
    }

    #[test]
    fn every_glyph_in_the_shipped_table_is_one_cell() {
        for glyph in GLYPHS.0.exact.values().chain(GLYPHS.0.prefixes.iter().map(|(_, glyph)| glyph)) {
            assert_eq!(qframe::text::width(&glyph.to_string()), 1, "U+{:04X}", u32::from(*glyph));
        }
    }

    #[test]
    fn an_exact_name_wins_over_a_prefix_and_a_longer_prefix_over_a_shorter_one() {
        let table = table(
            "[icons]\n\"python-*\" = \"e73c\"\n\"python-pip\" = \"f0487\"\n\"py-*\" = \"e606\"\n\"python-django-*\" = \"e71d\"  # dev-django\n",
        );
        assert_eq!(table.lookup("python-pip"), Some('\u{f0487}'));
        assert_eq!(table.lookup("python-requests"), Some('\u{e73c}'));
        assert_eq!(table.lookup("python-django-rest"), Some('\u{e71d}'));
        assert_eq!(table.lookup("pyside"), None);
        assert_eq!(table.lookup("py-thing"), Some('\u{e606}'));
    }

    #[test]
    fn builds_of_the_same_program_share_its_glyph() {
        let table = table(
            "[icons]\n\"firefox\" = \"e745\"\n\"itch-setup-bin\" = \"ef99\"\n\"mesa\" = \"f0379\"\n\"fire*\" = \"f0238\"\n",
        );
        assert_eq!(table.lookup("firefox-bin"), Some(FIREFOX));
        assert_eq!(table.lookup("firefox-git"), Some(FIREFOX), "the exact name without -git beats the fire* prefix");
        assert_eq!(table.lookup("lib32-mesa"), Some('\u{f0379}'));
        assert_eq!(table.lookup("lib32-mesa-git"), Some('\u{f0379}'));
        assert_eq!(table.lookup("itch-setup-bin"), Some('\u{ef99}'), "a name listed with its ending is found as it is");
        assert_eq!(table.lookup("firewall"), Some('\u{f0238}'));
    }

    #[test]
    fn flatpak_ids_are_looked_up_like_names() {
        assert_eq!(GLYPHS.0.lookup("org.mozilla.firefox"), Some(FIREFOX));
        assert_eq!(GLYPHS.0.lookup("firefox"), Some(FIREFOX));
        assert_eq!(GLYPHS.0.lookup("com.example.Nothing"), None);
    }

    #[test]
    fn a_broken_line_is_reported_where_it_breaks_and_the_rest_still_loads() {
        let text = "# fine\n[icons]\n\"a\" = \"zz\"\n\"b\" \"e745\"\nc = \"e745\"\n\"d\" = \"e745\" trailing\n\"*\" = \"e745\"\n\"e\" = \"e745\"\n\"e\" = \"f303\"\n[other]\n\"ok\" = \"e745\"\n";
        let (table, problems) = GlyphTable::parse("test.toml", text);
        let places: Vec<String> =
            problems.iter().filter_map(|p| p.location.as_ref().map(ToString::to_string)).collect();
        assert_eq!(
            places,
            [
                "test.toml:3:7",
                "test.toml:4:5",
                "test.toml:5:1",
                "test.toml:6:14",
                "test.toml:7:1",
                "test.toml:9:1",
                "test.toml:10:1",
                "test.toml:11:1"
            ],
            "{problems:?}"
        );
        assert_eq!(table.lookup("e"), Some(FIREFOX), "a key given twice keeps its first glyph");
        assert_eq!(table.lookup("ok"), None, "entries under an unknown section are not taken");
    }

    #[test]
    fn a_code_point_wider_than_a_cell_is_refused() {
        let (table, problems) = GlyphTable::parse("test.toml", "[icons]\n\"wide\" = \"1f600\"\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(table.lookup("wide"), None);
    }

    #[test]
    fn the_kind_and_source_icons_load_cleanly_and_are_one_cell_in_every_mode() {
        let mut registry = IconSetRegistry::builtin();
        assert!(registry.add_source(SET_ID, SET.0, SET.1));
        assert!(registry.diagnostics().is_empty(), "{:?}", registry.diagnostics());
        let kinds = Category::BROWSABLE.into_iter().chain([Category::Library, Category::Unknown]);
        let keys: Vec<String> = kinds
            .map(|kind| format!("category.{}", kind.key()))
            .chain(crate::sources::ALL.map(|source| format!("source.{}", crate::sources::name(source))))
            .collect();
        for icons in SET_ICONS.iter() {
            for key in &keys {
                assert!(icons.contains(key), "{key}");
                assert_eq!(qframe::text::width(&icons.glyph(key)), 1, "{key} in {:?}", icons.mode());
            }
        }
    }

    #[test]
    fn the_table_speaks_only_in_nerd_mode_then_the_kind_then_the_source() {
        use GlyphMode::{Ascii, Nerd, Unicode};
        let firefox = |mode| glyph("firefox", Category::Internet, Source::Pacman, mode);
        assert_eq!(firefox(Nerd), FIREFOX.to_string());
        assert_eq!(firefox(Unicode), "◎", "the kind's icon where the font has no Nerd glyphs");
        assert_eq!(firefox(Ascii), "@");
        let obs = |mode| glyph("obs-vkcapture", Category::AudioVideo, Source::Pacman, mode);
        assert_eq!([obs(Nerd), obs(Unicode), obs(Ascii)], ["\u{f040c}", "▶", ">"], "not in the table: its kind");
        let unknown = |source, mode| glyph("obscure-tool", Category::Unknown, source, mode);
        assert_eq!(
            [unknown(Source::Pacman, Nerd), unknown(Source::Pacman, Unicode), unknown(Source::Pacman, Ascii)],
            ["\u{f03d3}", "◉", "o"],
            "no kind: its source"
        );
        assert_eq!(unknown(Source::Flatpak, Unicode), "◫");
        assert_eq!(unknown(Source::Aur, Ascii), "a");
    }

    #[test]
    fn an_installed_package_takes_its_kind_from_its_groups_and_its_name() {
        let package = |name: &str, groups: &[&str]| Package {
            name: name.to_owned(),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..Package::default()
        };
        assert_eq!(installed(&package("libxml2", &[]), GlyphMode::Unicode), "▣");
        assert_eq!(installed(&package("font-misc", &["xorg-fonts"]), GlyphMode::Ascii), "A");
        assert_eq!(installed(&package("git", &[]), GlyphMode::Nerd), "\u{e702}", "the table first in Nerd mode");
        assert_eq!(installed(&package("git", &[]), GlyphMode::Unicode), "◉", "unplaced: the repositories' icon");
    }
}

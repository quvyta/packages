//! The installed-package table: which packages are shown, in what order, and as which rows.
//!
//! The table draws what it is given; filtering and sorting happen here, only when the search, the
//! view or the sort changes, never per frame. The rows themselves are built the first time a
//! frame needs them and kept until the packages, the glyph mode, the width class or the language
//! change, because a row carries the source's name in the active language.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::widgets::{Column, ColumnWidth, SortDirection, TableCell, TableRow};
use qpackages_core::pacman::Package;

use super::Library;
use super::query::Query;
use crate::icons;

/// Where an installed pacman package came from, as far as the local database and the
/// repositories tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// One of the repositories offers it.
    Repo,
    /// No repository offers it: on Arch that is a package built from the AUR, or by hand.
    Aur,
}

/// What is known about which installed packages no repository offers: `None` until the query
/// answered, or when it could not run, in which case no package claims a source.
pub type Foreign = Option<BTreeSet<String>>;

/// The origin of the package named `name`, when `foreign` is known.
#[must_use]
pub fn origin(foreign: &Foreign, name: &str) -> Option<Origin> {
    foreign.as_ref().map(|foreign| if foreign.contains(name) { Origin::Aur } else { Origin::Repo })
}

/// A column of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    /// The package's icon and name.
    Name,
    /// The installed version.
    Version,
    /// Where it came from.
    Source,
    /// The installed size.
    Size,
    /// Whether it was asked for, as opposed to pulled in as a dependency.
    Explicit,
}

/// The columns on an ordinary screen.
const WIDE: [Col; 5] = [Col::Name, Col::Version, Col::Source, Col::Size, Col::Explicit];

/// The columns on a very narrow screen: the source goes, since a shortened source name would
/// mean nothing.
const NARROW: [Col; 4] = [Col::Name, Col::Version, Col::Size, Col::Explicit];

/// The columns shown, narrow or not.
#[must_use]
pub fn visible(narrow: bool) -> &'static [Col] {
    if narrow { &NARROW } else { &WIDE }
}

/// The titles of `cols`, in the active language.
#[must_use]
pub fn columns(cols: &[Col]) -> Vec<Column> {
    cols.iter()
        .map(|col| match col {
            Col::Name => Column::new(t!("packages.name")).min(10).sortable(true),
            Col::Version => Column::new(t!("packages.version")).min(8),
            Col::Source => Column::new(t!("installed.source")).width(ColumnWidth::Fixed(8)).sortable(true),
            Col::Size => {
                Column::new(t!("packages.size")).width(ColumnWidth::Fixed(10)).align(Align::End).sortable(true)
            }
            Col::Explicit => Column::new(t!("packages.explicit")).width(ColumnWidth::Fixed(8)).sortable(true),
        })
        .collect()
}

/// Which packages a table may show at all: every one, or only those in `apps`.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    /// The packages that are applications, when only those are shown.
    pub apps: Option<&'a BTreeSet<String>>,
    /// Which packages no repository offers.
    pub foreign: &'a Foreign,
}

/// The indices into `packages` of those in `scope` matching `query`, sorted by `col` in
/// `direction`. Ties, and columns that do not sort, fall back to name order so the order is
/// always stable.
#[must_use]
pub fn shown(packages: &[Package], scope: Scope<'_>, query: &Query, col: Col, direction: SortDirection) -> Vec<usize> {
    let mut shown: Vec<usize> = (0..packages.len())
        .filter(|&i| {
            let package = &packages[i];
            scope.apps.is_none_or(|apps| apps.contains(&package.name))
                && query.matches(package, origin(scope.foreign, &package.name))
        })
        .collect();
    shown.sort_by(|&a, &b| {
        let (a, b) = (&packages[a], &packages[b]);
        let order = match col {
            Col::Name => a.name.cmp(&b.name),
            Col::Size => a.size.cmp(&b.size),
            // Asked-for packages first: they are the ones the user chose.
            Col::Explicit => b.explicit.cmp(&a.explicit),
            // The repositories first; a package whose source is unknown goes last.
            Col::Source => {
                let (a, b) = (origin(scope.foreign, &a.name), origin(scope.foreign, &b.name));
                a.is_none().cmp(&b.is_none()).then(a.cmp(&b))
            }
            Col::Version => Ordering::Equal,
        };
        let order = if direction == SortDirection::Descending { order.reverse() } else { order };
        order.then_with(|| a.name.cmp(&b.name))
    });
    shown
}

/// What a set of built rows depends on besides the packages: when any of it changes, the rows
/// are built again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowsKey {
    /// The glyph mode, which decides whether the glyph table speaks.
    pub mode: GlyphMode,
    /// Whether the source column is left out.
    pub narrow: bool,
    /// The words for the two origins and the orphans' mark, in the active language.
    pub labels: [String; 3],
}

impl RowsKey {
    /// The key for drawing in `mode`, `narrow` or not, with the active language's words.
    #[must_use]
    pub fn new(mode: GlyphMode, narrow: bool) -> Self {
        Self { mode, narrow, labels: [t!("installed.origin-repo"), t!("installed.origin-aur"), t!("installed.orphan")] }
    }
}

/// The rows last built, and what they were built for. Emptied whenever the shown packages change.
#[derive(Debug, Default)]
pub struct Rows(RefCell<Option<(RowsKey, Arc<[TableRow]>)>>);

impl Rows {
    /// Forgets the rows; the next frame builds them again.
    pub fn clear(&mut self) {
        *self.0.get_mut() = None;
    }

    /// The rows for `key`: the kept ones when they were built for it, otherwise those `build`
    /// makes, which are then kept.
    pub fn get(&self, key: RowsKey, build: impl FnOnce(&RowsKey) -> Arc<[TableRow]>) -> Arc<[TableRow]> {
        let mut kept = self.0.borrow_mut();
        if let Some((built_for, rows)) = kept.as_ref()
            && *built_for == key
        {
            return Arc::clone(rows);
        }
        let rows = build(&key);
        *kept = Some((key, Arc::clone(&rows)));
        rows
    }
}

/// The table rows for `shown` of `library`, in that order, as `key` asks.
///
/// The name starts with the package's icon: a quiet glyph one space before the name, which takes
/// the row's text colour while the row is selected, and stays whole when the column is too narrow
/// for the name.
///
/// An orphan's row is faint and says so where a package asked for would have its check: an
/// orphan is never asked for, so the column is free.
#[must_use]
pub fn rows(library: Library<'_>, shown: &[usize], key: &RowsKey) -> Arc<[TableRow]> {
    let cols = visible(key.narrow);
    let (packages, foreign) = (library.packages, library.foreign);
    shown
        .iter()
        .map(|&i| {
            let package = &packages[i];
            let orphan = library.orphans.as_ref().is_some_and(|orphans| orphans.contains(&package.name));
            TableRow::new(cols.iter().map(|col| match col {
                Col::Name => TableCell::new(package.name.clone()).icon(icons::installed(package, key.mode), None),
                Col::Version => TableCell::new(package.version.clone()),
                Col::Source => TableCell::new(match origin(foreign, &package.name) {
                    Some(Origin::Repo) => key.labels[0].clone(),
                    Some(Origin::Aur) => key.labels[1].clone(),
                    None => String::new(),
                }),
                Col::Size => TableCell::new(package.size.map(size_text).unwrap_or_default()),
                Col::Explicit if package.explicit => TableCell::new("").icon("check", None),
                Col::Explicit if orphan => TableCell::new(key.labels[2].clone()),
                Col::Explicit => TableCell::new(""),
            }))
            .faint(orphan)
        })
        .collect()
}

/// A size in the units pacman prints, one decimal from KiB up.
#[must_use]
pub fn size_text(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let units = [("KiB", KIB), ("MiB", KIB * KIB), ("GiB", KIB * KIB * KIB)];
    // Sizes are far below 2^53, so the conversion to f64 is exact.
    let bytes_f = bytes as f64;
    match units.iter().rev().find(|(_, unit)| bytes_f >= *unit) {
        Some((name, unit)) => format!("{:.1} {name}", bytes_f / unit),
        None => format!("{bytes} B"),
    }
}

#[cfg(test)]
mod tests {
    use qframe::widgets::Table;

    use super::*;

    fn package(name: &str, size: u64, explicit: bool, description: &str) -> Package {
        Package {
            name: name.to_owned(),
            version: "1.0-1".to_owned(),
            description: Some(description.to_owned()),
            size: Some(size),
            explicit,
            ..Package::default()
        }
    }

    fn three() -> Vec<Package> {
        vec![
            package("bash", 10_000, true, "The GNU Bourne Again shell"),
            package("glibc", 50_000, false, "GNU C Library"),
            package("zsh", 20_000, true, "A very advanced and programmable command interpreter"),
        ]
    }

    const UNKNOWN: Foreign = None;

    fn all(foreign: &Foreign) -> Scope<'_> {
        Scope { apps: None, foreign }
    }

    fn find(packages: &[Package], search: &str, col: Col, direction: SortDirection) -> Vec<usize> {
        shown(packages, all(&UNKNOWN), &Query::parse(search), col, direction)
    }

    #[test]
    fn the_search_looks_at_names_and_descriptions_without_caring_about_case() {
        let packages = three();
        assert_eq!(find(&packages, "", Col::Name, SortDirection::Ascending), [0, 1, 2]);
        assert_eq!(find(&packages, "SH", Col::Name, SortDirection::Ascending), [0, 2]);
        assert_eq!(find(&packages, "gnu", Col::Name, SortDirection::Ascending), [0, 1]);
        assert_eq!(find(&packages, "nothing like it", Col::Name, SortDirection::Ascending), []);
    }

    #[test]
    fn sorting_by_size_and_status_falls_back_to_the_name() {
        let packages = three();
        assert_eq!(find(&packages, "", Col::Size, SortDirection::Ascending), [0, 2, 1]);
        assert_eq!(find(&packages, "", Col::Size, SortDirection::Descending), [1, 2, 0]);
        assert_eq!(find(&packages, "", Col::Explicit, SortDirection::Ascending), [0, 2, 1], "asked-for first");
        assert_eq!(find(&packages, "", Col::Explicit, SortDirection::Descending), [1, 0, 2]);
        assert_eq!(find(&packages, "", Col::Name, SortDirection::Descending), [2, 1, 0]);
        assert_eq!(
            find(&packages, "", Col::Version, SortDirection::Descending),
            [0, 1, 2],
            "the version column keeps name order"
        );
    }

    #[test]
    fn sorting_by_source_puts_the_repositories_first_and_the_unknown_last() {
        let packages = three();
        let foreign: Foreign = Some(BTreeSet::from(["bash".to_owned()]));
        let by_source = |direction| shown(&packages, all(&foreign), &Query::parse(""), Col::Source, direction);
        assert_eq!(by_source(SortDirection::Ascending), [1, 2, 0]);
        assert_eq!(by_source(SortDirection::Descending), [0, 1, 2]);
        assert_eq!(origin(&UNKNOWN, "bash"), None, "nothing is claimed before the query answered");
    }

    #[test]
    fn the_apps_scope_keeps_only_the_applications() {
        let packages = three();
        let apps = BTreeSet::from(["zsh".to_owned()]);
        let scope = Scope { apps: Some(&apps), foreign: &UNKNOWN };
        assert_eq!(shown(&packages, scope, &Query::parse(""), Col::Name, SortDirection::Ascending), [2]);
    }

    #[test]
    fn sizes_read_like_pacman_prints_them() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1023), "1023 B");
        assert_eq!(size_text(1024), "1.0 KiB");
        assert_eq!(size_text(10_055_508), "9.6 MiB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn rows_are_kept_until_what_they_were_built_for_changes() {
        let mut rows = Rows::default();
        let key =
            |mode| RowsKey { mode, narrow: false, labels: ["Repo".to_owned(), "AUR".to_owned(), "orphan".to_owned()] };
        let built = std::cell::Cell::new(0);
        let build = |_: &RowsKey| {
            built.set(built.get() + 1);
            Arc::from([])
        };
        let _ = rows.get(key(GlyphMode::Nerd), build);
        let _ = rows.get(key(GlyphMode::Nerd), build);
        assert_eq!(built.get(), 1, "the same key keeps the rows");
        let _ = rows.get(key(GlyphMode::Ascii), build);
        assert_eq!(built.get(), 2, "another glyph mode builds them again");
        rows.clear();
        let _ = rows.get(key(GlyphMode::Ascii), build);
        assert_eq!(built.get(), 3, "cleared rows are built again");
    }

    /// The package table alone, drawn the way the Installed tab draws it; the third field is the
    /// row the table shows as chosen.
    struct Installed(Vec<Package>, Foreign, Option<usize>);

    impl Installed {
        /// The table with nothing chosen.
        fn new(packages: Vec<Package>, foreign: Foreign) -> Self {
            Self(packages, foreign, None)
        }
    }

    impl App for Installed {
        type Msg = ();

        fn update(&mut self, (): ()) -> Command<()> {
            Command::none()
        }

        fn view(&self, ui: &mut View<'_, ()>) {
            let narrow = ui.size().width < 40;
            let key = RowsKey::new(ui.env().icons().mode(), narrow);
            let shown: Vec<usize> = (0..self.0.len()).collect();
            let apps = BTreeSet::new();
            let library = Library { packages: &self.0, apps: &apps, foreign: &self.1, orphans: &None };
            let rows = rows(library, &shown, &key);
            ui.add(Table::new(columns(visible(narrow)), rows).selected(self.2)).fill();
        }
    }

    #[test]
    fn a_name_icon_is_quiet_and_takes_the_name_colour_on_the_chosen_row() {
        let packages = vec![package("bash", 1024, true, ""), package("zsh", 1024, true, "")];
        let colours = |selected| {
            let app = Installed(packages.clone(), None, selected);
            let mut h = Harness::with_env(app, crate::test_env(), 80, 8);
            h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
            let (x, y) = h.find("bash").expect("the first row");
            let (x, y) = (u16::try_from(x).expect("on screen"), u16::try_from(y).expect("on screen"));
            // The glyph sits one space before the name it was found by.
            (h.fg(x - 2, y), h.fg(x, y))
        };
        let (icon, name) = colours(None);
        assert_ne!(icon, name, "at rest the glyph is quieter than the name");
        let (icon, name) = colours(Some(0));
        assert_eq!(icon, name, "on the chosen row a muted glyph would look disabled");
    }

    #[test]
    fn a_name_column_too_narrow_cuts_the_name_and_keeps_the_glyph() {
        let long = package("a-very-long-package-name-indeed", 1024, true, "");
        let mut h = Harness::with_env(Installed::new(vec![long], None), crate::test_env(), 36, 6);
        h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        let screen = h.screen();
        assert!(screen.contains('…'), "the name gives way:\n{screen}");
        assert!(screen.contains("◉ a-very"), "the glyph and its space stay:\n{screen}");
    }

    #[test]
    fn names_carry_their_icon_in_every_glyph_mode() {
        let packages: Vec<Package> = ["bash", "firefox-bin", "libxml2", "python-requests", "ttf-dejavu"]
            .into_iter()
            .map(|name| package(name, 1024, true, ""))
            .collect();
        let mut h = Harness::with_env(Installed::new(packages, None), crate::test_env(), 80, 8);
        h.set_locale("en").set_glyph_mode(GlyphMode::Nerd);
        let screen = h.screen();
        for line in [
            "\u{e760} bash",
            "\u{e745} firefox-bin",
            "\u{f03d6} libxml2",
            "\u{e73c} python-requests",
            "\u{f06d6} ttf-dejavu",
        ] {
            assert!(screen.contains(line), "`{line}` in Nerd mode:\n{screen}");
        }
        h.set_glyph_mode(GlyphMode::Ascii);
        let screen = h.screen();
        for line in ["o bash", "o firefox-bin", "# libxml2", "# python-requests", "A ttf-dejavu"] {
            assert!(screen.contains(line), "`{line}` in ASCII mode:\n{screen}");
        }
        h.set_glyph_mode(GlyphMode::Unicode);
        assert!(h.screen().contains("▣ libxml2"), "{}", h.screen());
    }

    #[test]
    fn the_source_reads_in_the_active_language_and_leaves_a_very_narrow_screen() {
        let packages = vec![package("paru", 1024, true, ""), package("bash", 1024, true, "")];
        let foreign: Foreign = Some(BTreeSet::from(["paru".to_owned()]));
        let mut h = Harness::with_env(Installed::new(packages, foreign), crate::test_env(), 70, 6);
        h.set_locale("en").set_glyph_mode(GlyphMode::Unicode);
        let screen = h.screen();
        assert!(screen.contains("Source") && screen.contains("AUR") && screen.contains("Repo"), "{screen}");
        h.resize(38, 6);
        let screen = h.screen();
        assert!(!screen.contains("AUR") && !screen.contains("Sourc"), "the column is gone:\n{screen}");
    }
}

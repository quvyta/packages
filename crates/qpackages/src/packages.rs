//! The package table: which packages are shown, in what order, and as which rows.
//!
//! The table draws what it is given; filtering and sorting happen here, and the rows are
//! rebuilt only when the search or the sort changes, never per frame.

use std::cmp::Ordering;
use std::sync::Arc;

use qframe::prelude::*;
use qframe::widgets::{Column, ColumnWidth, SortDirection, TableCell, TableRow};
use qpackages_core::pacman::Package;

/// The name column.
pub const NAME: usize = 0;
/// The installed size column.
pub const SIZE: usize = 2;
/// The column marking packages the user asked for, as opposed to dependencies.
pub const EXPLICIT: usize = 3;

/// The columns, titled in the active language.
#[must_use]
pub fn columns() -> [Column; 4] {
    [
        Column::new(t!("packages.name")).min(10).sortable(true),
        Column::new(t!("packages.version")).min(8),
        Column::new(t!("packages.size")).width(ColumnWidth::Fixed(10)).align(Align::End).sortable(true),
        Column::new(t!("packages.explicit")).width(ColumnWidth::Fixed(8)).sortable(true),
    ]
}

/// Whether `package` matches the `search` typed by the user: a case-insensitive part of its name
/// or of its description, the way `pacman -Qs` looks. An empty search matches everything.
#[must_use]
pub fn matches(package: &Package, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    let search = search.to_lowercase();
    package.name.to_lowercase().contains(&search)
        || package.description.as_deref().is_some_and(|text| text.to_lowercase().contains(&search))
}

/// The indices into `packages` of those matching `search`, sorted by `column` in `direction`.
/// Ties, and columns that do not sort, fall back to name order so the order is always stable.
#[must_use]
pub fn shown(packages: &[Package], search: &str, column: usize, direction: SortDirection) -> Vec<usize> {
    let mut shown: Vec<usize> = (0..packages.len()).filter(|&i| matches(&packages[i], search)).collect();
    shown.sort_by(|&a, &b| {
        let (a, b) = (&packages[a], &packages[b]);
        let order = match column {
            NAME => a.name.cmp(&b.name),
            SIZE => a.size.cmp(&b.size),
            // Asked-for packages first: they are the ones the user chose.
            EXPLICIT => b.explicit.cmp(&a.explicit),
            _ => Ordering::Equal,
        };
        let order = if direction == SortDirection::Descending { order.reverse() } else { order };
        order.then_with(|| a.name.cmp(&b.name))
    });
    shown
}

/// The table rows for `shown`, in that order.
///
/// The cells carry no translated words: the rows outlive a language change, since they are
/// only rebuilt when the search or the sort changes, so what a cell says must read the same in
/// every language. Whether a package was asked for is a mark under a translated title.
#[must_use]
pub fn rows(packages: &[Package], shown: &[usize]) -> Arc<[TableRow]> {
    shown
        .iter()
        .map(|&i| {
            let package = &packages[i];
            let explicit = if package.explicit { TableCell::new("").icon("check", None) } else { TableCell::new("") };
            TableRow::new([
                TableCell::new(package.name.clone()),
                TableCell::new(package.version.clone()),
                TableCell::new(package.size.map(size_text).unwrap_or_default()),
                explicit,
            ])
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

    #[test]
    fn the_search_looks_at_names_and_descriptions_without_caring_about_case() {
        let packages = three();
        assert_eq!(shown(&packages, "", NAME, SortDirection::Ascending), [0, 1, 2]);
        assert_eq!(shown(&packages, "SH", NAME, SortDirection::Ascending), [0, 2]);
        assert_eq!(shown(&packages, "gnu", NAME, SortDirection::Ascending), [0, 1]);
        assert_eq!(shown(&packages, "nothing like it", NAME, SortDirection::Ascending), []);
    }

    #[test]
    fn sorting_by_size_and_status_falls_back_to_the_name() {
        let packages = three();
        assert_eq!(shown(&packages, "", SIZE, SortDirection::Ascending), [0, 2, 1]);
        assert_eq!(shown(&packages, "", SIZE, SortDirection::Descending), [1, 2, 0]);
        assert_eq!(shown(&packages, "", EXPLICIT, SortDirection::Ascending), [0, 2, 1], "asked-for first");
        assert_eq!(shown(&packages, "", EXPLICIT, SortDirection::Descending), [1, 0, 2]);
        assert_eq!(shown(&packages, "", NAME, SortDirection::Descending), [2, 1, 0]);
        assert_eq!(
            shown(&packages, "", 1, SortDirection::Descending),
            [0, 1, 2],
            "the version column keeps name order"
        );
    }

    #[test]
    fn sizes_read_like_pacman_prints_them() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1023), "1023 B");
        assert_eq!(size_text(1024), "1.0 KiB");
        assert_eq!(size_text(10_055_508), "9.6 MiB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}

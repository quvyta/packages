//! The order the application shows the sources in, and the short name each goes by in the
//! settings file and the language files.

use qpackages_core::sources::Source;

/// Every source, in sidebar order.
pub const ALL: [Source; 4] = [Source::Pacman, Source::Aur, Source::Flatpak, Source::Snap];

/// The package in the official repositories that brings a source to this machine, for the
/// sources this version can install itself. Snap's package lives in the AUR, which a later
/// version manages, so it has none here.
#[must_use]
pub const fn package(source: Source) -> Option<&'static str> {
    match source {
        Source::Aur => Some("paru"),
        Source::Flatpak => Some("flatpak"),
        Source::Pacman | Source::Snap => None,
    }
}

/// The AUR package that brings a source this version cannot install from the repositories.
/// snapd is not in the official repositories, so Snap is offered as an AUR build like any other.
#[must_use]
pub const fn aur_package(source: Source) -> Option<&'static str> {
    match source {
        Source::Snap => Some("snapd"),
        Source::Pacman | Source::Aur | Source::Flatpak => None,
    }
}

/// The name a source goes by under `sources.` in the settings file and `source.` in the
/// language files.
#[must_use]
pub const fn name(source: Source) -> &'static str {
    match source {
        Source::Pacman => "pacman",
        Source::Aur => "aur",
        Source::Flatpak => "flatpak",
        Source::Snap => "snap",
    }
}

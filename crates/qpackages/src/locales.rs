//! The language files and the key bindings, compiled in so an installed binary needs nothing
//! beside it, and the environment the runtime and the tests load them into.
//!
//! A new language is one line in [`LOCALES`]: the file is compiled in, the checks in
//! `locales/tests.rs` pick it up, and every screen test that runs in every language runs in it.

use qframe::env::AssetDirs;

/// The compiled-in language files. English comes first: it is the fallback, and every other file
/// is held to its keys.
pub const LOCALES: &[(&str, &str)] = &[
    ("en.toml", include_str!("../assets/locales/en.toml")),
    ("tr.toml", include_str!("../assets/locales/tr.toml")),
    ("de.toml", include_str!("../assets/locales/de.toml")),
    ("es.toml", include_str!("../assets/locales/es.toml")),
    ("fr.toml", include_str!("../assets/locales/fr.toml")),
    ("pt-BR.toml", include_str!("../assets/locales/pt-BR.toml")),
    ("ru.toml", include_str!("../assets/locales/ru.toml")),
    ("zh-Hans.toml", include_str!("../assets/locales/zh-Hans.toml")),
    ("ja.toml", include_str!("../assets/locales/ja.toml")),
];

/// The application's own key bindings, compiled in for the same reason.
pub const KEYMAP: (&str, &str) = ("keymap.toml", include_str!("../assets/keymap.toml"));

/// The compiled-in language files, key bindings and icon set, as the runtime loads them, for
/// drawing the screen outside a terminal: `qframe::env::Env::load` turns them into the
/// environment a test harness takes.
#[must_use]
pub fn asset_dirs() -> AssetDirs {
    AssetDirs {
        locale_sources: LOCALES.iter().map(|(file, text)| ((*file).to_owned(), (*text).to_owned())).collect(),
        keymap_source: Some((KEYMAP.0.to_owned(), KEYMAP.1.to_owned())),
        icon_sources: vec![(crate::icons::SET.0.to_owned(), crate::icons::SET.1.to_owned())],
        ..AssetDirs::default()
    }
}

/// The environment as the runtime loads it, for tests that drive the screen.
#[cfg(test)]
pub(crate) fn env() -> qframe::env::Env {
    qframe::env::Env::load(&asset_dirs()).expect("the locales and the keymap are readable")
}

#[cfg(test)]
pub(crate) mod tests;

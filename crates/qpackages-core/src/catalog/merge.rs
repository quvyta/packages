//! One card per application, however many sources offer it.
//!
//! The repositories' AppStream catalog and Flathub's share ids, so an application found in both
//! becomes one entry offering both. The AUR has no AppStream data: an AUR package joins a
//! repository application only when its name is that application's package name, once a `-bin`
//! or `-git` ending is dropped, and only when exactly one application has that package. Anything
//! less certain stays a card of its own; a wrong merge is worse than two cards.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::appstream::{Component, Icon, Localized};
use super::aur::AurPackage;
use super::category::Category;
use crate::sources::Source;

/// The sources from most to least trusted, the order offers are listed and installs default to:
/// signed repository packages, sandboxed Flatpaks, Snaps, then user-submitted AUR recipes.
pub const TRUST_ORDER: [Source; 4] = [Source::Pacman, Source::Flatpak, Source::Snap, Source::Aur];

/// One way to install an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Where it comes from.
    pub source: Source,
    /// What to install: a package name for pacman and the AUR, an application id for Flatpak.
    pub package: String,
}

/// A repository package, as the sync database or `pacman -Ss` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoPackage {
    /// The package name.
    pub name: String,
    /// The one-line description.
    pub description: Option<String>,
    /// The pacman groups it belongs to.
    pub groups: Vec<String>,
}

/// One entry of the store: an application and every source that offers it.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    /// The AppStream id, when a catalog knows the application.
    pub id: Option<String>,
    /// The name on the card.
    pub name: Localized,
    /// The summary under the name.
    pub summary: Option<Localized>,
    /// The kind it is sorted under.
    pub category: Category,
    /// The icon its catalog names.
    pub icon: Option<Icon>,
    /// Search keywords in the default language.
    pub keywords: Vec<String>,
    /// Search keywords in Turkish.
    pub keywords_turkish: Vec<String>,
    /// Every way to install it, most trusted first.
    pub offers: Vec<Offer>,
}

impl App {
    /// Orders the offers by `order`; a source missing from `order` goes last. The order is stable,
    /// so two offers from one source keep the order they were found in.
    pub fn sort_offers(&mut self, order: &[Source]) {
        let rank = |source: Source| order.iter().position(|&known| known == source).unwrap_or(order.len());
        self.offers.sort_by_key(|offer| rank(offer.source));
    }

    /// Whether `source` offers it.
    #[must_use]
    pub fn offered_by(&self, source: Source) -> bool {
        self.offers.iter().any(|offer| offer.source == source)
    }

    fn from_component(component: &Component, offer: Offer) -> Self {
        Self {
            id: Some(component.id.clone()),
            name: component.name.clone(),
            summary: component.summary.clone(),
            category: category_of(component),
            icon: component.icon.clone(),
            keywords: component.keywords.clone(),
            keywords_turkish: component.keywords_turkish.clone(),
            offers: vec![offer],
        }
    }

    fn from_package(name: &str, description: Option<&str>, groups: &[String], offer: Offer) -> Self {
        Self {
            id: None,
            name: Localized { default: name.to_owned(), turkish: None },
            summary: description.map(|default| Localized { default: default.to_owned(), turkish: None }),
            category: Category::classify::<&str, _>(&[], groups, name),
            icon: None,
            keywords: Vec::new(),
            keywords_turkish: Vec::new(),
            offers: vec![offer],
        }
    }

    /// Fills what this entry lacks from another catalog's record of the same application.
    fn complete_from(&mut self, component: &Component) {
        if self.name.turkish.is_none() {
            self.name.turkish.clone_from(&component.name.turkish);
        }
        match (&mut self.summary, &component.summary) {
            (None, Some(summary)) => self.summary = Some(summary.clone()),
            (Some(own), Some(other)) if own.turkish.is_none() => own.turkish.clone_from(&other.turkish),
            _ => {}
        }
        if self.category == Category::Unknown {
            self.category = category_of(component);
        }
        if self.icon.is_none() {
            self.icon.clone_from(&component.icon);
        }
        if self.keywords_turkish.is_empty() {
            self.keywords_turkish.clone_from(&component.keywords_turkish);
        }
    }
}

/// The key two AppStream ids are compared by. Ids are reverse domain names, where case carries
/// no meaning (`org.videolan.vlc` in Arch, `org.videolan.VLC` on Flathub), and older catalogs
/// end them in `.desktop` (`com.valvesoftware.Steam.desktop`).
#[must_use]
pub fn id_key(id: &str) -> String {
    let lower = id.to_lowercase();
    match lower.strip_suffix(".desktop") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

/// The name an AUR package would have in the repositories: without a `-bin` or `-git` ending.
#[must_use]
pub fn aur_stem(name: &str) -> &str {
    name.strip_suffix("-bin").or_else(|| name.strip_suffix("-git")).unwrap_or(name)
}

/// Combines the sources into one entry per application, in the order they were first seen:
/// repository applications, Flathub applications, other repository packages, then AUR packages.
///
/// Only components the store lists as cards are taken (see
/// [`Kind::is_listed`](super::appstream::Kind::is_listed)); an add-on's package still appears
/// through `repo_packages`. A repository component without a package name offers nothing and is
/// left out. A repository package that an application already offers does not get a second card.
#[must_use]
pub fn merge(repo: &[Component], flathub: &[Component], repo_packages: &[RepoPackage], aur: &[AurPackage]) -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    // Which entries offer each repository package; an AUR package joins only a sole owner.
    let mut by_package: HashMap<String, Vec<usize>> = HashMap::new();

    for component in repo.iter().filter(|component| component.kind.is_listed()) {
        let Some(pkgname) = &component.pkgname else { continue };
        let offer = Offer { source: Source::Pacman, package: pkgname.clone() };
        let index = match by_id.entry(id_key(&component.id)) {
            Entry::Occupied(known) => {
                let app = &mut apps[*known.get()];
                if app.offers.contains(&offer) {
                    continue;
                }
                app.offers.push(offer);
                *known.get()
            }
            Entry::Vacant(slot) => {
                slot.insert(apps.len());
                apps.push(App::from_component(component, offer));
                apps.len() - 1
            }
        };
        by_package.entry(pkgname.clone()).or_default().push(index);
    }

    for component in flathub.iter().filter(|component| component.kind.is_listed()) {
        let offer = Offer { source: Source::Flatpak, package: component.id.clone() };
        match by_id.entry(id_key(&component.id)) {
            Entry::Occupied(known) => {
                let app = &mut apps[*known.get()];
                if !app.offers.contains(&offer) {
                    app.offers.push(offer);
                    app.complete_from(component);
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(apps.len());
                apps.push(App::from_component(component, offer));
            }
        }
    }

    for package in repo_packages {
        if let Entry::Vacant(slot) = by_package.entry(package.name.clone()) {
            slot.insert(vec![apps.len()]);
            let offer = Offer { source: Source::Pacman, package: package.name.clone() };
            apps.push(App::from_package(&package.name, package.description.as_deref(), &package.groups, offer));
        }
    }

    for package in aur {
        let offer = Offer { source: Source::Aur, package: package.name.clone() };
        match by_package.get(aur_stem(&package.name)).map(Vec::as_slice) {
            Some(&[only]) => apps[only].offers.push(offer),
            _ => apps.push(App::from_package(&package.name, package.description.as_deref(), &[], offer)),
        }
    }

    for app in &mut apps {
        app.sort_offers(&TRUST_ORDER);
    }
    apps
}

/// The kind a catalog's component belongs to.
fn category_of(component: &Component) -> Category {
    if component.kind == super::appstream::Kind::Font {
        return Category::Fonts;
    }
    Category::from_freedesktop(&component.categories)
        .or_else(|| component.pkgname.as_deref().and_then(Category::from_name))
        .unwrap_or(Category::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{appstream, aur};

    const ARCH: &str = include_str!("../../tests/fixtures/catalog/arch-extra.xml");
    const FLATHUB: &str = include_str!("../../tests/fixtures/catalog/flathub.xml");
    const AUR_SEARCH: &str = include_str!("../../tests/fixtures/catalog/aur-search-obs.json");
    const AUR_INFO: &str = include_str!("../../tests/fixtures/catalog/aur-info.json");

    fn recorded() -> Vec<App> {
        let repo = appstream::parse(ARCH).0.components;
        let flathub = appstream::parse(FLATHUB).0.components;
        let mut aur = aur::parse_response(AUR_SEARCH).expect("recorded search");
        aur.extend(aur::parse_response(AUR_INFO).expect("recorded info"));
        let packages = [
            RepoPackage {
                name: String::from("obs-studio"),
                description: Some(String::from("Free software")),
                groups: vec![],
            },
            RepoPackage { name: String::from("libxml2"), description: None, groups: vec![] },
            RepoPackage {
                name: String::from("zynaddsubfx"),
                description: None,
                groups: vec![String::from("pro-audio")],
            },
        ];
        merge(&repo, &flathub, &packages, &aur)
    }

    fn component(id: &str, pkgname: Option<&str>) -> Component {
        Component {
            id: id.to_owned(),
            kind: appstream::Kind::DesktopApplication,
            pkgname: pkgname.map(str::to_owned),
            name: Localized { default: id.to_owned(), turkish: None },
            summary: None,
            categories: vec![],
            keywords: vec![],
            keywords_turkish: vec![],
            icon: None,
            license: None,
            homepage: None,
            description: None,
        }
    }

    fn find<'a>(apps: &'a [App], package: &str) -> &'a App {
        apps.iter()
            .find(|app| app.offers.iter().any(|offer| offer.package == package))
            .unwrap_or_else(|| panic!("something offers {package}"))
    }

    fn sources(app: &App) -> Vec<(Source, &str)> {
        app.offers.iter().map(|offer| (offer.source, offer.package.as_str())).collect()
    }

    #[test]
    fn the_same_id_in_the_repositories_and_on_flathub_is_one_card() {
        let apps = recorded();
        let gimp = find(&apps, "gimp");
        assert_eq!(sources(gimp), [(Source::Pacman, "gimp"), (Source::Flatpak, "org.gimp.GIMP")]);
        assert_eq!(gimp.category, Category::Graphics);
        assert_eq!(gimp.name.default, "GNU Image Manipulation Program", "the repository's record leads");
        let vlc = find(&apps, "vlc");
        assert_eq!(
            sources(vlc),
            [(Source::Pacman, "vlc"), (Source::Flatpak, "org.videolan.VLC")],
            "case does not matter"
        );
    }

    #[test]
    fn a_desktop_ending_does_not_keep_the_same_id_apart() {
        assert_eq!(id_key("com.valvesoftware.Steam.desktop"), id_key("com.valvesoftware.Steam"));
        assert_eq!(id_key("org.videolan.vlc"), "org.videolan.vlc");
    }

    #[test]
    fn different_ids_stay_different_cards() {
        let apps = recorded();
        // Arch calls it obsidian.desktop, Flathub md.obsidian.Obsidian: not provably the same.
        assert_eq!(sources(find(&apps, "obsidian")), [(Source::Pacman, "obsidian")]);
        assert_eq!(sources(find(&apps, "md.obsidian.Obsidian")), [(Source::Flatpak, "md.obsidian.Obsidian")]);
    }

    #[test]
    fn an_aur_build_of_a_repository_package_joins_its_card_last() {
        let apps = recorded();
        let obs = find(&apps, "obs-studio");
        assert_eq!(
            sources(obs),
            [
                (Source::Pacman, "obs-studio"),
                (Source::Flatpak, "com.obsproject.Studio"),
                (Source::Aur, "obs-studio-git")
            ]
        );
        assert_eq!(
            apps.iter().filter(|app| app.offered_by(Source::Pacman) && app.offers[0].package == "obs-studio").count(),
            1
        );
    }

    #[test]
    fn an_aur_package_with_another_name_stays_its_own_card() {
        let apps = recorded();
        for name in ["obs-studio-tytan652", "obs-studio-liberty", "obs-vkcapture", "visual-studio-code-bin"] {
            assert_eq!(sources(find(&apps, name)), [(Source::Aur, name)], "{name}");
        }
        let code = find(&apps, "visual-studio-code-bin");
        assert_eq!(code.id, None);
        assert!(code.summary.is_some());
    }

    #[test]
    fn an_aur_package_matching_two_cards_joins_neither() {
        let repo = [component("adljack.desktop", Some("adljack")), component("adlrt.desktop", Some("adljack"))];
        let aur = [AurPackage { name: String::from("adljack-git"), ..AurPackage::default() }];
        let apps = merge(&repo, &[], &[], &aur);
        assert_eq!(apps.len(), 3);
        assert_eq!(sources(&apps[2]), [(Source::Aur, "adljack-git")]);
    }

    #[test]
    fn repository_packages_without_a_catalog_entry_get_plain_cards() {
        let apps = recorded();
        let libxml = find(&apps, "libxml2");
        assert_eq!(libxml.category, Category::Library, "from the name");
        assert_eq!(find(&apps, "zynaddsubfx").category, Category::AudioVideo, "from the group");
        assert_eq!(
            apps.iter().filter(|app| app.offers.iter().any(|offer| offer.package == "obs-studio")).count(),
            1,
            "a package an application already offers gets no second card"
        );
    }

    #[test]
    fn add_ons_and_runtimes_are_not_cards_but_fonts_are() {
        let apps = recorded();
        assert!(apps.iter().all(|app| app.id.as_deref() != Some("codeblocks-contrib")));
        assert!(apps.iter().all(|app| app.id.as_deref() != Some("org.freedesktop.Platform")));
        assert_eq!(find(&apps, "gsfonts").category, Category::Fonts);
    }

    #[test]
    fn flathub_fills_in_a_missing_translation() {
        let mut repo = component("org.example.Tool", Some("tool"));
        repo.summary = Some(Localized { default: String::from("A tool"), turkish: None });
        let mut flathub = component("org.example.tool", None);
        flathub.name.turkish = Some(String::from("Araç"));
        flathub.summary = Some(Localized { default: String::from("A tool"), turkish: Some(String::from("Bir araç")) });
        flathub.categories = vec![String::from("Utility")];
        let apps = merge(&[repo], &[flathub], &[], &[]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name.get(true), "Araç");
        assert_eq!(apps[0].summary.as_ref().map(|summary| summary.get(true)), Some("Bir araç"));
        assert_eq!(apps[0].category, Category::Utility, "an unknown kind is taken from the other catalog");
    }

    #[test]
    fn offers_follow_any_order_asked_for() {
        let mut app = find(&recorded(), "obs-studio").clone();
        app.sort_offers(&[Source::Aur, Source::Flatpak]);
        let order: Vec<Source> = app.offers.iter().map(|offer| offer.source).collect();
        assert_eq!(order, [Source::Aur, Source::Flatpak, Source::Pacman]);
    }

    #[test]
    fn stems_drop_one_known_ending() {
        assert_eq!(aur_stem("obs-studio-git"), "obs-studio");
        assert_eq!(aur_stem("visual-studio-code-bin"), "visual-studio-code");
        assert_eq!(aur_stem("obs-studio-tytan652"), "obs-studio-tytan652");
        assert_eq!(aur_stem("foo-bin-git"), "foo-bin", "only one ending is dropped");
    }
}

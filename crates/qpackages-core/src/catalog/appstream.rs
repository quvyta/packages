//! Reading AppStream collection files, the catalogs software centres are built from.
//!
//! Arch ships them as `archlinux-appstream-data` (`/usr/share/swcatalog/xml/*.xml.gz`) and
//! Flatpak keeps one per remote (`/var/lib/flatpak/appstream/<remote>/<arch>/active/`). Both are
//! a `<components>` root holding one `<component>` per application, font or add-on. Only what the
//! store shows is kept: names and summaries in the default language and in Turkish, categories,
//! keywords, an icon name, the licence, the home page and the description's text. Screenshots,
//! releases and the description's markup are skipped; a catalog of 1,600 components is 29 MB of
//! text, most of it those.
//!
//! Each component is read on its own. A component that is not well-formed, or lacks an id or a
//! name, is skipped and reported with its position; the ones around it are unaffected.

use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::Problem;

/// One catalog file, read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Catalog {
    /// Where the catalog says it comes from, such as `archlinux-arch-extra` or `flatpak`.
    pub origin: Option<String>,
    /// The components that could be read, in file order.
    pub components: Vec<Component>,
}

/// What a component is, from its `type` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// An application with a window and a desktop entry.
    DesktopApplication,
    /// A program run in a terminal.
    ConsoleApplication,
    /// A web application wrapped as a desktop one.
    WebApplication,
    /// A font family.
    Font,
    /// A plugin or extension for another component.
    Addon,
    /// A runtime other applications run on, as Flatpak has.
    Runtime,
    /// Any other type, kept as written: `inputmethod`, `icon-theme`, `service` and the like.
    Other(String),
}

impl Kind {
    /// The kind a `type` attribute names. A component without the attribute is generic.
    fn from_type(text: &str) -> Self {
        match text {
            // `desktop` is the older name for the same thing, still found in Flathub's catalog.
            "desktop-application" | "desktop" => Self::DesktopApplication,
            "console-application" => Self::ConsoleApplication,
            "web-application" => Self::WebApplication,
            "font" => Self::Font,
            "addon" => Self::Addon,
            "runtime" => Self::Runtime,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Whether the store shows a component of this kind as a card of its own. Add-ons belong on
    /// the page of what they extend, and runtimes are nothing a person installs on purpose.
    #[must_use]
    pub const fn is_listed(&self) -> bool {
        matches!(self, Self::DesktopApplication | Self::ConsoleApplication | Self::WebApplication | Self::Font)
    }
}

/// A text in the catalog's default language, with its Turkish translation when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Localized {
    /// The untranslated text, usually English.
    pub default: String,
    /// The text marked `xml:lang="tr"` (or `tr_TR`, `tr-TR`).
    pub turkish: Option<String>,
}

impl Localized {
    /// The Turkish text when `turkish` is asked for and there is one, otherwise the default.
    #[must_use]
    pub fn get(&self, turkish: bool) -> &str {
        match &self.turkish {
            Some(text) if turkish => text,
            _ => &self.default,
        }
    }
}

/// The icon a component names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    /// A name to look up in the icon theme, such as `firefox`.
    Stock(String),
    /// A file name in the catalog's own icon cache, beside the XML file.
    Cached(String),
}

/// One application, font or add-on in a catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// The AppStream id, such as `org.mozilla.firefox`. Flathub uses the same ids, which is what
    /// lets the same application from two sources become one card.
    pub id: String,
    /// What the component is.
    pub kind: Kind,
    /// The package that installs it. Distribution catalogs have one; Flathub's do not, since
    /// there the id is what is installed.
    pub pkgname: Option<String>,
    /// The name shown on the card.
    pub name: Localized,
    /// The one-line summary under the name.
    pub summary: Option<Localized>,
    /// The freedesktop categories, as written: `AudioVideo`, `Recorder`, …
    pub categories: Vec<String>,
    /// Search keywords in the default language.
    pub keywords: Vec<String>,
    /// Search keywords in Turkish.
    pub keywords_turkish: Vec<String>,
    /// The first stock or cached icon; remote icons are left out, the store does not download.
    pub icon: Option<Icon>,
    /// The licence as an SPDX expression, as written.
    pub license: Option<String>,
    /// The project's home page.
    pub homepage: Option<String>,
    /// The long description as plain text: one line per paragraph or list item, markup and
    /// wrapping removed.
    pub description: Option<Localized>,
}

/// Reads a whole AppStream collection file.
///
/// Returns every component that could be read and every problem met. Components are found by
/// their start tag and read one at a time, so a broken one ends at the next `<component` and
/// cannot take the rest of the catalog with it.
#[must_use]
pub fn parse(xml: &str) -> (Catalog, Vec<Problem>) {
    let starts = component_starts(xml);
    let mut problems = Vec::new();
    let head = &xml[..starts.first().copied().unwrap_or(xml.len())];
    let origin = read_origin(head).unwrap_or_else(|(offset, message)| {
        problems.push(Problem::at(xml, offset, message));
        None
    });
    let mut components = Vec::with_capacity(starts.len());
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(xml.len());
        match read_component(&xml[start..end]) {
            Ok(component) => components.push(component),
            Err((offset, message)) => problems.push(Problem::at(xml, start + offset, message)),
        }
    }
    (Catalog { origin, components }, problems)
}

/// The byte offsets of every `<component` start tag. `<components` is the root and does not
/// count: the character after the name must end it.
fn component_starts(xml: &str) -> Vec<usize> {
    const TAG: &str = "<component";
    xml.match_indices(TAG)
        .filter(|(at, _)| {
            xml[at + TAG.len()..].starts_with(|next: char| next == '>' || next == '/' || next.is_whitespace())
        })
        .map(|(at, _)| at)
        .collect()
}

/// A problem found while reading: the byte offset it is at, and what it is.
type Failure = (usize, String);

/// The `origin` attribute of the `<components>` root, from the text before the first component.
fn read_origin(head: &str) -> Result<Option<String>, Failure> {
    let mut reader = Reader::from_str(head);
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag) | Event::Empty(tag)) if tag.local_name().as_ref() == "components" => {
                return attribute(&tag, "origin").map_err(|message| (position(&reader), message));
            }
            // A catalog with no components ends here, with or without its root closed.
            Ok(Event::Eof) => return Ok(None),
            Ok(_) => {}
            Err(error) => return Err((error_position(&reader), error.to_string())),
        }
    }
}

/// Which language an element is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Default,
    Turkish,
    Other,
}

impl Lang {
    /// The language an `xml:lang` value names. `C` is how some generators write the default.
    fn from_attribute(value: Option<&str>) -> Self {
        match value {
            None | Some("C") => Self::Default,
            Some(tag) if tag == "tr" || tag.starts_with("tr_") || tag.starts_with("tr-") => Self::Turkish,
            Some(_) => Self::Other,
        }
    }
}

/// The element whose text is being collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Id,
    Pkgname,
    Name(Lang),
    Summary(Lang),
    License,
    StockIcon,
    CachedIcon,
    Homepage,
    Category,
    Keyword(Lang),
    /// A paragraph or a list item of the description.
    Description(Lang),
}

/// What a component has gathered so far.
#[derive(Default)]
struct Draft {
    id: Option<String>,
    kind: Option<Kind>,
    pkgname: Option<String>,
    name: Option<String>,
    name_turkish: Option<String>,
    summary: Option<String>,
    summary_turkish: Option<String>,
    categories: Vec<String>,
    keywords: Vec<String>,
    keywords_turkish: Vec<String>,
    icon: Option<Icon>,
    license: Option<String>,
    homepage: Option<String>,
    description: Vec<String>,
    description_turkish: Vec<String>,
}

impl Draft {
    /// Stores the text of a finished field. The first value of a single field wins, the way
    /// software centres read these files.
    fn take(&mut self, field: Field, text: String) {
        if text.is_empty() {
            return;
        }
        let slot = match field {
            Field::Id => &mut self.id,
            Field::Pkgname => &mut self.pkgname,
            Field::Name(Lang::Default) => &mut self.name,
            Field::Name(Lang::Turkish) => &mut self.name_turkish,
            Field::Summary(Lang::Default) => &mut self.summary,
            Field::Summary(Lang::Turkish) => &mut self.summary_turkish,
            Field::License => &mut self.license,
            Field::Homepage => &mut self.homepage,
            Field::StockIcon | Field::CachedIcon => {
                if self.icon.is_none() {
                    self.icon = Some(if field == Field::StockIcon { Icon::Stock(text) } else { Icon::Cached(text) });
                }
                return;
            }
            Field::Category => return self.categories.push(text),
            Field::Keyword(Lang::Default) => return self.keywords.push(text),
            Field::Keyword(Lang::Turkish) => return self.keywords_turkish.push(text),
            Field::Description(Lang::Default) => return self.description.push(text),
            Field::Description(Lang::Turkish) => return self.description_turkish.push(text),
            Field::Name(Lang::Other)
            | Field::Summary(Lang::Other)
            | Field::Keyword(Lang::Other)
            | Field::Description(Lang::Other) => return,
        };
        slot.get_or_insert(text);
    }

    /// The component, or why it cannot be one.
    fn finish(self) -> Result<Component, String> {
        let Some(id) = self.id else {
            let described = self.name.map_or_else(String::new, |name| format!(" `{name}`"));
            return Err(format!("component{described} has no <id> and is skipped"));
        };
        let Some(name) = self.name else {
            return Err(format!("component `{id}` has no <name> and is skipped"));
        };
        // A translation that is not whole would mix languages in one text; it is left out.
        let description = (!self.description.is_empty()).then(|| Localized {
            turkish: (self.description_turkish.len() == self.description.len())
                .then(|| self.description_turkish.join("\n")),
            default: self.description.join("\n"),
        });
        Ok(Component {
            id,
            kind: self.kind.unwrap_or_else(|| Kind::Other(String::from("generic"))),
            pkgname: self.pkgname,
            name: Localized { default: name, turkish: self.name_turkish },
            summary: self.summary.map(|default| Localized { default, turkish: self.summary_turkish }),
            categories: self.categories,
            keywords: self.keywords,
            keywords_turkish: self.keywords_turkish,
            icon: self.icon,
            license: self.license,
            homepage: self.homepage,
            description,
        })
    }
}

/// Reads the one component `text` starts with, stopping at its end tag.
fn read_component(text: &str) -> Result<Component, Failure> {
    let mut reader = Reader::from_str(text);
    let mut draft = Draft::default();
    let mut depth = 0_usize;
    // The field being collected, its text so far and the depth of its element: markup inside it,
    // such as `<em>` in a paragraph, adds its text and does not end it.
    let mut capture: Option<(Field, String, usize)> = None;
    // The languages of the `<keywords>` list and of the `<description>` being read.
    let mut keywords: Option<Lang> = None;
    let mut description: Option<Lang> = None;
    let mut in_categories = false;
    let fail = |reader: &Reader<&[u8]>, draft: &Draft, message: String| {
        let named = draft.id.as_ref().map_or_else(String::new, |id| format!(" `{id}`"));
        (position(reader), format!("component{named} is skipped: {message}"))
    };
    loop {
        let event = reader.read_event().map_err(|error| {
            let named = draft.id.as_ref().map_or_else(String::new, |id| format!(" `{id}`"));
            (error_position(&reader), format!("component{named} is skipped: {error}"))
        })?;
        match event {
            Event::Start(tag) => {
                depth += 1;
                let name = tag.local_name();
                let lang = || attribute(&tag, "xml:lang").map(|value| Lang::from_attribute(value.as_deref()));
                let field = match (depth, name.as_ref()) {
                    (1, "component") => {
                        let kind = attribute(&tag, "type").map_err(|message| fail(&reader, &draft, message))?;
                        draft.kind = Some(
                            kind.map_or_else(|| Kind::Other(String::from("generic")), |kind| Kind::from_type(&kind)),
                        );
                        None
                    }
                    (1, _) => {
                        let found = name.as_ref().to_owned();
                        return Err(fail(&reader, &draft, format!("expected <component>, found <{found}>")));
                    }
                    (2, "id") => Some(Field::Id),
                    (2, "pkgname") => Some(Field::Pkgname),
                    (2, "name") => Some(Field::Name(lang().map_err(|message| fail(&reader, &draft, message))?)),
                    (2, "summary") => Some(Field::Summary(lang().map_err(|message| fail(&reader, &draft, message))?)),
                    (2, "project_license") => Some(Field::License),
                    (2, "icon") => {
                        match attribute(&tag, "type").map_err(|message| fail(&reader, &draft, message))?.as_deref() {
                            Some("stock") => Some(Field::StockIcon),
                            Some("cached") => Some(Field::CachedIcon),
                            _ => None,
                        }
                    }
                    (2, "url") => {
                        let kind = attribute(&tag, "type").map_err(|message| fail(&reader, &draft, message))?;
                        (kind.as_deref() == Some("homepage")).then_some(Field::Homepage)
                    }
                    (2, "categories") => {
                        in_categories = true;
                        None
                    }
                    (2, "keywords") => {
                        keywords = Some(lang().map_err(|message| fail(&reader, &draft, message))?);
                        None
                    }
                    (2, "description") => {
                        description = Some(lang().map_err(|message| fail(&reader, &draft, message))?);
                        None
                    }
                    (3, "category") if in_categories => Some(Field::Category),
                    (3, "p") | (4, "li") if capture.is_none() => description.map(|whole| {
                        // Catalogs translate paragraph by paragraph, each with its own language.
                        match attribute(&tag, "xml:lang") {
                            Ok(Some(own)) => Field::Description(Lang::from_attribute(Some(&own))),
                            _ => Field::Description(whole),
                        }
                    }),
                    (3, "keyword") => keywords.map(|list| {
                        // A keyword may carry its own language instead of the list's.
                        match attribute(&tag, "xml:lang") {
                            Ok(Some(own)) => Field::Keyword(Lang::from_attribute(Some(&own))),
                            _ => Field::Keyword(list),
                        }
                    }),
                    _ => None,
                };
                if let Some(field) = field {
                    capture = Some((field, String::new(), depth));
                }
            }
            Event::Empty(tag) if depth == 0 && tag.local_name().as_ref() == "component" => {
                return Err(fail(&reader, &draft, String::from("the component is empty")));
            }
            Event::Text(text) => {
                if let Some((_, collected, _)) = capture.as_mut() {
                    collected.push_str(&text.xml10_content());
                }
            }
            Event::CData(data) => {
                if let Some((_, collected, _)) = capture.as_mut() {
                    collected.push_str(&data.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((_, collected, _)) = capture.as_mut() {
                    let written = format!("&{};", reference.xml10_content());
                    let resolved = unescape(&written).map_err(|error| fail(&reader, &draft, error.to_string()))?;
                    collected.push_str(&resolved);
                }
            }
            Event::End(tag) => {
                match (depth, tag.local_name().as_ref()) {
                    (2, "categories") => in_categories = false,
                    (2, "keywords") => keywords = None,
                    (2, "description") => description = None,
                    _ => {}
                }
                if capture.as_ref().is_some_and(|(_, _, at)| *at == depth)
                    && let Some((field, collected, _)) = capture.take()
                {
                    draft.take(field, normalize_space(&collected));
                }
                depth -= 1;
                if depth == 0 {
                    return draft.finish().map_err(|message| (0, message));
                }
            }
            Event::Eof => {
                return Err(fail(
                    &reader,
                    &draft,
                    String::from("the file or the next component starts before </component>"),
                ));
            }
            _ => {}
        }
    }
}

/// The value of attribute `key`, unescaped, or `None` when the tag does not have it.
fn attribute(tag: &BytesStart<'_>, key: &str) -> Result<Option<String>, String> {
    match tag.try_get_attribute(key) {
        Ok(Some(found)) => found
            .normalized_value(XmlVersion::Implicit1_0)
            .map(|value| Some(value.into_owned()))
            .map_err(|error| error.to_string()),
        Ok(None) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Collapses runs of whitespace to one space and trims the ends: catalogs wrap long summaries
/// over several indented lines.
fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the reader is, as a byte offset into its text.
fn position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX)
}

/// Where the reader's last error is, as a byte offset into its text.
fn error_position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.error_position()).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::gzip;

    const ARCH: &str = include_str!("../../tests/fixtures/catalog/arch-extra.xml");
    const FLATHUB: &str = include_str!("../../tests/fixtures/catalog/flathub.xml");

    fn find<'a>(catalog: &'a Catalog, id: &str) -> &'a Component {
        catalog
            .components
            .iter()
            .find(|component| component.id == id)
            .unwrap_or_else(|| panic!("{id} is in the catalog"))
    }

    #[test]
    fn reads_the_recorded_arch_catalog() {
        let (catalog, problems) = parse(ARCH);
        assert_eq!(catalog.origin.as_deref(), Some("archlinux-arch-extra"));
        assert_eq!(catalog.components.len(), 20, "22 components, two of them broken");
        assert_eq!(problems.len(), 2, "{problems:?}");

        let firefox = find(&catalog, "org.mozilla.firefox");
        assert_eq!(firefox.kind, Kind::DesktopApplication);
        assert_eq!(firefox.pkgname.as_deref(), Some("firefox"));
        assert_eq!(firefox.name.default, "Firefox");
        let summary = firefox.summary.as_ref().expect("firefox has a summary");
        assert_eq!(summary.default, "Fast, Private & Safe Web Browser", "the entity is resolved");
        assert_eq!(firefox.categories, ["Network", "WebBrowser"]);
        assert_eq!(firefox.icon, Some(Icon::Cached(String::from("firefox_firefox.jxl"))));
        assert_eq!(firefox.homepage.as_deref(), Some("https://www.firefox.com/"));
        assert_eq!(firefox.license.as_deref(), Some("MPL-2.0"));
    }

    #[test]
    fn keeps_the_turkish_text_beside_the_default() {
        let (catalog, _) = parse(ARCH);
        let gimp = find(&catalog, "org.gimp.GIMP");
        assert_eq!(gimp.name.default, "GNU Image Manipulation Program");
        assert_eq!(gimp.name.turkish.as_deref(), Some("GNU Görüntü İşleme Programı"));
        assert_eq!(gimp.name.get(true), "GNU Görüntü İşleme Programı");
        assert_eq!(gimp.name.get(false), "GNU Image Manipulation Program");
        let summary = gimp.summary.as_ref().expect("gimp has a summary");
        assert_eq!(summary.turkish.as_deref(), Some("Yüksek nitelikli görüntü oluşturma ve işleme"));
        assert!(gimp.keywords.iter().any(|keyword| keyword == "Photoshop"), "{:?}", gimp.keywords);
        let firefox = find(&catalog, "org.mozilla.firefox");
        assert_eq!(firefox.keywords, ["mozilla", "internet", "web"]);
        assert!(firefox.keywords_turkish.iter().any(|keyword| keyword == "Tarayıcı"), "{:?}", firefox.keywords_turkish);

        let acme = find(&catalog, "acme.desktop");
        assert_eq!(acme.name.turkish, None);
        assert_eq!(acme.name.get(true), "Acme", "without a translation the default is shown");
    }

    #[test]
    fn a_name_inside_another_element_is_not_the_components_name() {
        let (catalog, _) = parse(ARCH);
        let kart = find(&catalog, "net.supertuxkart.SuperTuxKart");
        assert_eq!(kart.name.default, "SuperTuxKart", "the developer's <name> is one level deeper");
        assert_eq!(
            kart.summary.as_ref().and_then(|summary| summary.turkish.as_deref()),
            Some("3D açık kaynaklı kart yarış oyunu")
        );
    }

    #[test]
    fn reads_fonts_addons_and_console_programs() {
        let (catalog, _) = parse(ARCH);
        let font = find(&catalog, "de.urwpp.D050000L");
        assert_eq!(font.kind, Kind::Font);
        assert_eq!(font.pkgname.as_deref(), Some("gsfonts"));
        assert!(font.kind.is_listed());
        let addon = find(&catalog, "codeblocks-contrib");
        assert_eq!(addon.kind, Kind::Addon);
        assert!(!addon.kind.is_listed());
        assert_eq!(find(&catalog, "io.github.syllo.nvtop").kind, Kind::ConsoleApplication);
    }

    #[test]
    fn a_broken_component_is_skipped_and_reported_where_it_is() {
        let (catalog, problems) = parse(ARCH);
        assert!(catalog.components.iter().all(|component| component.id != "org.example.Broken"));
        let misspelt = ARCH.find("</sumary>").expect("the fixture has the broken tag");
        let line = ARCH[..misspelt].matches('\n').count() + 1;
        assert_eq!(problems[0].line, line, "{}", problems[0]);
        assert!(problems[0].message.contains("org.example.Broken"), "{}", problems[0]);

        let nameless = ARCH.find("<name>No id</name>").expect("the fixture has the component without an id");
        let start = ARCH[..nameless].rfind("<component").expect("its start tag");
        assert_eq!(problems[1].line, ARCH[..start].matches('\n').count() + 1, "{}", problems[1]);
        assert!(problems[1].message.contains("no <id>"), "{}", problems[1]);

        let after = ["org.gimp.GIMP", "org.videolan.vlc"];
        assert!(
            after.iter().all(|id| catalog.components.iter().any(|component| component.id == *id)),
            "the rest survives"
        );
    }

    #[test]
    fn reads_the_recorded_flathub_catalog() {
        let (catalog, problems) = parse(FLATHUB);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(catalog.origin.as_deref(), Some("flatpak"));
        let vlc = find(&catalog, "org.videolan.VLC");
        assert_eq!(vlc.pkgname, None, "Flathub installs by id, there is no package name");
        assert_eq!(vlc.icon, Some(Icon::Cached(String::from("org.videolan.VLC.png"))));
        assert!(vlc.name.turkish.is_some());
        let runtime = find(&catalog, "org.freedesktop.Platform");
        assert_eq!(runtime.kind, Kind::Runtime);
    }

    #[test]
    fn the_compressed_catalog_reads_the_same_as_the_plain_one() {
        let bytes =
            gzip::decompress(include_bytes!("../../tests/fixtures/catalog/arch-extra.xml.gz")).expect("valid gzip");
        let text = std::str::from_utf8(&bytes).expect("the catalog is UTF-8");
        assert_eq!(parse(text), parse(ARCH));
    }

    #[test]
    fn only_turkish_counts_as_turkish() {
        assert_eq!(Lang::from_attribute(Some("tr")), Lang::Turkish);
        assert_eq!(Lang::from_attribute(Some("tr_TR")), Lang::Turkish);
        assert_eq!(Lang::from_attribute(Some("tr-TR")), Lang::Turkish);
        assert_eq!(Lang::from_attribute(Some("trs")), Lang::Other, "Chicahuaxtla Triqui is not Turkish");
        assert_eq!(Lang::from_attribute(Some("C")), Lang::Default);
        assert_eq!(Lang::from_attribute(None), Lang::Default);
    }

    #[test]
    fn wrapped_text_is_joined_onto_one_line() {
        let xml = "<components>\n<component type=\"font\">\n  <id>a</id>\n  <name>A</name>\n  <summary>one\n     two</summary>\n</component>\n</components>";
        let (catalog, problems) = parse(xml);
        assert!(problems.is_empty());
        assert_eq!(catalog.components[0].summary.as_ref().map(|summary| summary.default.as_str()), Some("one two"));
    }

    #[test]
    fn the_description_is_read_as_plain_lines_with_its_translation() {
        let xml = "<components><component type=\"desktop-application\"><id>a</id><name>A</name>\n\
            <description>\n  <p>Streams and <em>records</em>\n  video.</p>\n  <p xml:lang=\"tr\">Yayın ve kayıt.</p>\n\
            <ul><li>Scenes</li><li xml:lang=\"tr\">Sahneler</li></ul>\n</description>\n\
            <summary>After the description</summary></component></components>";
        let (catalog, problems) = parse(xml);
        assert!(problems.is_empty(), "{problems:?}");
        let component = &catalog.components[0];
        let description = component.description.as_ref().expect("a description");
        assert_eq!(description.default, "Streams and records video.\nScenes", "inline markup keeps its text");
        assert_eq!(description.turkish.as_deref(), Some("Yayın ve kayıt.\nSahneler"));
        assert_eq!(component.summary.as_ref().map(|summary| summary.default.as_str()), Some("After the description"));
        assert_eq!(
            find(&parse(ARCH).0, "org.kernel.software.network.ethtool")
                .description
                .as_ref()
                .map(|d| d.turkish.is_none()),
            Some(true)
        );
    }

    #[test]
    fn a_partial_translation_of_the_description_is_left_out() {
        let xml = "<components><component><id>a</id><name>A</name><description><p>One</p><p>Two</p>\
            <p xml:lang=\"tr\">Bir</p></description></component></components>";
        let description = parse(xml).0.components[0].description.clone().expect("a description");
        assert_eq!(description, Localized { default: String::from("One\nTwo"), turkish: None });
    }

    #[test]
    fn bad_input_never_panics() {
        let samples = [
            "",
            "not xml at all",
            "<components>",
            "<components><component>",
            "<components><component type=\"x\"><id>a</id>",
            "<components><component/></components>",
            "<components><component type='a><id>a</id></component>",
            "<component><id>&bogus;</id><name>x</name></component>",
            "<component><id>a</id><name>x</name></component></component></component>",
        ];
        for sample in samples {
            let (catalog, _) = parse(sample);
            assert!(catalog.components.len() <= 1, "{sample}");
        }
        // Every cut of a real catalog reads without panicking.
        for end in (0..ARCH.len()).step_by(97).filter(|&end| ARCH.is_char_boundary(end)) {
            let _ = parse(&ARCH[..end]);
        }
    }

    #[test]
    fn a_catalog_without_components_is_empty_not_an_error() {
        let xml =
            "<?xml version=\"1.0\"?>\n<components version=\"1.0\" origin=\"archlinux-arch-core\">\n</components>\n";
        let (catalog, problems) = parse(xml);
        assert!(problems.is_empty());
        assert_eq!(catalog.origin.as_deref(), Some("archlinux-arch-core"));
        assert!(catalog.components.is_empty());
    }
}

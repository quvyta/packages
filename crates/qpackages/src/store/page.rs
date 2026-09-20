//! One application's page: what it is, what it takes, and the button that installs or removes it
//! from the source chosen.

use qframe::prelude::*;
use qframe::widgets::{ScrollView, Select};
use qpackages_core::catalog::appstream::Component;
use qpackages_core::catalog::aur::AurPackage;
use qpackages_core::catalog::repo::RepoInfo;
use qpackages_core::sources::Source;

use super::view::{NARROW, back_button};
use super::{Details, Msg, OpenApp, Request, Store};
use crate::detail::date_text;
use crate::icons;
use crate::installed::table::size_text;

/// How many dependencies are named before the rest are counted.
const DEPENDENCIES_NAMED: usize = 5;

/// Draws the open application's page.
pub fn show(store: &Store, ui: &mut View<'_, Msg>) {
    let Some(open) = &store.open else { return };
    let turkish = ui.env().i18n().active().starts_with("tr");
    let component =
        store.loaded.as_ref().zip(open.card.app.id.as_deref()).and_then(|(loaded, id)| loaded.component(id));
    ui.add_with(ScrollView::new(), |ui| {
        ui.column(|ui| {
            back_button(ui);
            header(store, open, turkish, ui);
            if let Some(description) = description(open, component, turkish) {
                ui.add(Text::new(description)).fill_width();
            }
            facts(open, component, ui);
        })
        .gap(1)
        .padding(Padding::symmetric(0, 2))
        .fill_width();
    })
    .fill();
}

/// Icon, name and summary on the left; the source choice and the main button on the right.
fn header(store: &Store, open: &OpenApp, turkish: bool, ui: &mut View<'_, Msg>) {
    let card = &open.card;
    let drawing = ui.env().icons();
    let icon = icons::glyph(card.icon_name(), card.app.category, card.first_source(), drawing.mode())
        .resolve(drawing)
        .into_owned();
    ui.row(|ui| {
        ui.column(|ui| {
            ui.add(
                Text::rich([Span::new(format!("{icon}  ")).color("muted"), Span::new(card.name(turkish)).bold()])
                    .role("title")
                    .no_wrap(),
            )
            .fill_width();
            if let Some(summary) = card.summary(turkish) {
                ui.add(Text::new(summary).color("muted")).fill_width();
            }
        })
        .fill_width();
        ui.column(|ui| {
            if card.app.offers.len() > 1 {
                let names = card.app.offers.iter().map(|offer| source_name(offer.source));
                ui.row(|ui| {
                    ui.add(Text::new(t!("store.app.source")).color("muted").no_wrap());
                    ui.add(Select::new(names).selected(Some(open.offer)).on_select(Msg::Offer)).id("store-source");
                })
                .gap(1);
            }
            main_button(store, open, ui);
        })
        .gap(1);
    })
    .gap(2)
    .fill_width();
}

/// Install, or Remove in the danger tone when the chosen source's package is installed. An AUR
/// package this machine has no paru or yay to build says so in place of the button, and which to
/// install.
fn main_button(store: &Store, open: &OpenApp, ui: &mut View<'_, Msg>) {
    let Some(offer) = open.card.app.offers.get(open.offer) else { return };
    let installed = store.installed.has(offer);
    let no_builder = store.sources.as_ref().is_some_and(|sources| sources.aur_helper.is_none());
    if offer.source == Source::Aur && !installed && no_builder {
        ui.add(Text::new(t!("store.app.aur-needs-helper")).color("warning")).fill_width().id("store-main");
        return;
    }
    let button = if installed {
        Button::new(t!("store.app.remove"))
            .variant("danger")
            .on_press(Msg::Request(Request::Remove(vec![offer.clone()])))
    } else {
        Button::new(t!("store.app.install"))
            .variant("primary")
            .on_press(Msg::Request(Request::Install(vec![offer.clone()])))
    };
    ui.add(button).id("store-main");
}

fn source_name(source: Source) -> String {
    t!(&format!("store.source.{}", crate::sources::name(source)))
}

/// The long description: the catalog's, else the one line the package gives when it says more
/// than the summary.
fn description(open: &OpenApp, component: Option<&Component>, turkish: bool) -> Option<String> {
    if let Some(description) = component.and_then(|component| component.description.as_ref()) {
        return Some(description.get(turkish).to_owned());
    }
    let line = match open.details.get(open.offer) {
        Some(Some(Ok(Details::Repo(info)))) => info.description.clone(),
        Some(Some(Ok(Details::Aur(package)))) => package.description.clone(),
        // snapd carries a whole description, which is the only long text a snap has.
        Some(Some(Ok(Details::Snap(snap)))) => snap.summary.clone(),
        _ => None,
    }?;
    let summary = open.card.summary(turkish);
    (summary != Some(line.as_str())).then_some(line)
}

/// The facts of the chosen source, two to a row where there is room.
fn facts(open: &OpenApp, component: Option<&Component>, ui: &mut View<'_, Msg>) {
    let Some(offer) = open.card.app.offers.get(open.offer) else { return };
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut warning = None;
    // The dependencies run long; they take a row of their own and wrap there.
    let mut depends = None;
    match (offer.source, open.details.get(open.offer)) {
        (Source::Pacman, Some(Some(Ok(Details::Repo(info))))) => depends = Some(repo_fields(info, &mut fields)),
        (Source::Aur, Some(Some(Ok(Details::Aur(package))))) => {
            depends = Some(aur_fields(package, &mut fields));
            warning = package.out_of_date.map(|since| t!("store.app.out-of-date", date = date_text(since)));
        }
        (Source::Flatpak, _) => {
            fields.push((t!("store.app.flatpak-id"), offer.package.clone()));
            if let Some(component) = component {
                fields.extend(component.license.clone().map(|license| (t!("store.app.license"), license)));
                fields.extend(component.homepage.as_deref().map(|url| (t!("store.app.website"), site(url))));
            }
        }
        (Source::Snap, Some(Some(Ok(Details::Snap(snap))))) => {
            fields.push((t!("store.app.snap-name"), snap.name.clone()));
            fields.push((t!("store.app.version"), snap.version.clone()));
            if let Some(publisher) = &snap.publisher {
                let shown =
                    if snap.verified { t!("snap.verified", who = publisher.as_str()) } else { publisher.clone() };
                fields.push((t!("store.app.publisher"), shown));
            }
            let size = snap.download_size.or(snap.installed_size);
            fields.extend(size.map(|bytes| (t!("store.app.download"), size_text(bytes))));
            if snap.is_classic() {
                warning = Some(t!("snap.classic-warning"));
            }
        }
        (Source::Pacman | Source::Aur | Source::Snap, Some(None) | None) => {
            ui.add(Text::new(t!("store.app.reading")).color("muted")).fill_width();
        }
        (Source::Pacman | Source::Aur | Source::Snap, Some(Some(_))) => {
            let glyph = ui.env().icons().glyph("warning").into_owned();
            ui.add(
                Text::new(format!("{glyph} {}", t!("store.app.unreachable", source = source_name(offer.source))))
                    .color("warning"),
            )
            .fill_width();
        }
    }
    if let Some(warning) = warning {
        let glyph = ui.env().icons().glyph("warning").into_owned();
        ui.add(Text::new(format!("{glyph} {warning}")).color("warning")).fill_width();
    }
    let pairs = if ui.size().width < NARROW { 1 } else { 2 };
    ui.column(|ui| {
        for chunk in fields.chunks(pairs) {
            ui.row(|ui| {
                for (label, value) in chunk {
                    ui.add(Text::rich([Span::new(format!("{label}  ")).color("muted"), Span::new(value.clone())]))
                        .fill_width();
                }
                if chunk.len() < pairs {
                    ui.spacer();
                }
            })
            .gap(2)
            .fill_width();
        }
        if let Some(depends) = depends {
            let label = t!("store.app.depends");
            ui.add(Text::rich([Span::new(format!("{label}  ")).color("muted"), Span::new(depends)])).fill_width();
        }
    })
    .fill_width();
}

/// The repository package's facts, and its dependencies as one text.
fn repo_fields(info: &RepoInfo, fields: &mut Vec<(String, String)>) -> String {
    fields.push((t!("store.app.version"), info.version.clone()));
    if !info.licenses.is_empty() {
        fields.push((t!("store.app.license"), info.licenses.join(", ")));
    }
    fields.extend(info.download_size.map(|size| (t!("store.app.download"), size_text(size))));
    fields.extend(info.installed_size.map(|size| (t!("store.app.on-disk"), size_text(size))));
    fields.push((t!("store.app.repository"), info.repo.clone()));
    fields.extend(info.url.as_deref().map(|url| (t!("store.app.website"), site(url))));
    dependencies(&info.depends)
}

/// The AUR package's facts, and its dependencies as one text.
fn aur_fields(package: &AurPackage, fields: &mut Vec<(String, String)>) -> String {
    fields.push((t!("store.app.version"), package.version.clone()));
    if !package.licenses.is_empty() {
        fields.push((t!("store.app.license"), package.licenses.join(", ")));
    }
    fields.push((t!("store.app.votes"), package.votes.to_string()));
    fields.push((t!("store.app.popularity"), format!("{:.2}", package.popularity)));
    let maintainer = package.maintainer.clone().unwrap_or_else(|| t!("store.app.orphaned"));
    fields.push((t!("store.app.maintainer"), maintainer));
    fields.extend(package.last_modified.map(|when| (t!("store.app.updated"), date_text(when))));
    fields.extend(package.url.as_deref().map(|url| (t!("store.app.website"), site(url))));
    dependencies(&package.depends)
}

/// The first dependencies by name and how many more there are.
fn dependencies(depends: &[String]) -> String {
    if depends.is_empty() {
        return t!("store.app.depends-none");
    }
    let named = depends.iter().take(DEPENDENCIES_NAMED).cloned().collect::<Vec<_>>().join(", ");
    match depends.len().saturating_sub(DEPENDENCIES_NAMED) {
        0 => named,
        more => t!("store.app.depends-more", names = named, n = more),
    }
}

/// A home page as a person reads it: the host and path, without the scheme or a trailing slash.
fn site(url: &str) -> String {
    let bare = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    bare.strip_suffix('/').unwrap_or(bare).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_website_drops_its_scheme_and_trailing_slash() {
        assert_eq!(site("https://obsproject.com"), "obsproject.com");
        assert_eq!(site("http://www.kernel.org/pub/"), "www.kernel.org/pub");
        assert_eq!(site("ftp://x"), "ftp://x");
    }
}

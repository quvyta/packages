//! The settings page: every setting qpac reads, one row each under a heading, saved the moment
//! it changes.
//!
//! Each heading is one function adding its heading and rows to the page's one list, so a new
//! group of settings is a new function and one line in [`view`]. The page holds no state of its
//! own: the values live in the application's settings, the row the keyboard is on in the list.

mod backend;
#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod tests;

use qframe::prelude::*;
use qframe::storage::Settings;
use qframe::widgets::{
    Appearance, AppearanceChange, ScrollView, Select, SettingRow, SettingsList, SettingsRows, Switch,
};
use qpackages_core::sources::{Availability, Source, Sources};

use crate::snap::State as SnapState;

use crate::helper::pkexec::Tool;
use crate::settings::{self, AUR_HELPERS, PRIVILEGE_TOOLS};
use crate::sources;

pub use backend::{Countries, Msg as BackendMsg, Reflector};

/// Columns a choice takes on the right of its row.
pub(crate) const CONTROL_WIDTH: u16 = 18;

/// The sources the page can turn on and off. pacman is not among them: without it there is
/// nothing to manage.
const SWITCHABLE: [Source; 3] = [Source::Aur, Source::Flatpak, Source::Snap];

/// Everything that can happen on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Back to the tab the page was opened from.
    Back,
    /// A source was turned on or off.
    Source(Source, bool),
    /// An AUR helper was chosen, by position in the settings' `AUR_HELPERS`.
    AurHelper(usize),
    /// A permission program was chosen, by position in the settings' `PRIVILEGE_TOOLS`.
    PrivilegeTool(usize),
    /// The user asked to install the program a source needs.
    Install(Source),
    /// The user asked to build a source's program from the AUR, since it is in no repository.
    BuildFromAur(Source),
    /// The user asked for snapd's socket to be switched on or off.
    SnapSocket(bool),
    /// The user asked to add Flathub as a Flatpak remote.
    AddFlathub,
    /// An appearance row was changed: a setting the family shares, or motion and the pillar.
    Appearance(AppearanceChange),
    /// A setting of what happens around the packages changed.
    Backend(BackendMsg),
}

/// What the page needs from the rest of the application to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Cx<'a> {
    /// The settings as they stand.
    pub settings: &'a Settings,
    /// Which sources this machine has, once the first read has said.
    pub sources: Option<&'a Sources>,
    /// Whether the user has Flathub as a Flatpak remote, once a read has said.
    pub flathub: Option<bool>,
    /// What snapd is, once a read has said; `None` while nothing has looked.
    pub snap: Option<&'a SnapState>,
    /// Whether qpac runs as root, which rules the AUR out.
    pub root: bool,
    /// Whether an installation is being planned, so the button that asked shows it.
    pub planning: bool,
    /// The program that asks for permission, as the setting and the machine decide.
    pub tool: &'a Tool,
    /// Which snapshot tools this machine has.
    pub detected: &'a qpackages_core::backup::Detected,
    /// Whether the background check can be switched: there is a unit folder and qpac's own path.
    pub background: bool,
    /// What is known about reflector.
    pub reflector: &'a Reflector,
    /// Whether a transaction is under way, so the settings that need the helper wait.
    pub busy: bool,
    /// The appearance rows, which save themselves.
    pub appearance: &'a Appearance,
}

/// Draws the page.
pub fn view(ui: &mut View<'_, Msg>, cx: Cx<'_>) {
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Button::new(t!("settings-page.back")).icon("arrow-left").on_press(Msg::Back)).id("settings-back");
            ui.add(Text::new(t!("settings-page.title")).role("title").no_wrap());
        })
        .gap(2)
        .fill_width();
        // One list for the whole page: the keys walk every setting in order, and a list taller
        // than the scroll view scrolls just enough to show the row they moved to, never on a click.
        ui.add_with(ScrollView::new(), |ui| {
            SettingsList::show(ui, |list| {
                sources_section(list, cx);
                backend::updates_section(list, cx);
                backend::backup_section(list, cx);
                backend::cleanup_section(list, cx);
                backend::mirrors_section(list, cx);
                privilege_section(list, cx);
                cx.appearance.section(list, Msg::Appearance);
            })
            .fill_width()
            .id("settings-list");
        })
        .fill();
        backend::countries(ui, cx);
    })
    .gap(1)
    .fill();
}

/// The sources: each on or off, or on offer to install when this machine lacks it, and the AUR
/// helper to prefer.
fn sources_section(list: &mut SettingsRows<'_, Msg>, cx: Cx<'_>) {
    list.heading(t!("settings-page.sources"));
    for source in SWITCHABLE {
        let name = t!(&format!("source.{}", sources::name(source)));
        let availability = cx.sources.map(|sources| sources.get(source));
        let enabled = settings::source_enabled(cx.settings, source);
        if source == Source::Aur && cx.root {
            let row = SettingRow::new(name).description(t!("source.not-as-root")).disabled(true);
            list.row(row, |ui| {
                ui.add(Switch::new(enabled).disabled(true));
            });
            continue;
        }
        match availability {
            Some(Availability::Missing) => missing_row(list, source, name, cx.planning),
            Some(Availability::Ready { .. }) | None => {
                let description = match (source, cx.sources) {
                    (Source::Aur, Some(found)) => found.aur_helper.map_or_else(
                        || t!("settings-page.installed"),
                        |helper| t!("settings-page.through", helper = helper.program()),
                    ),
                    // snapd's program being there is not enough: it only answers once its socket
                    // is switched on, which the Arch package leaves off.
                    (Source::Snap, _) => match cx.snap {
                        Some(state) => snap_description(state),
                        None => t!("settings-page.looking"),
                    },
                    (_, Some(_)) => t!("settings-page.installed"),
                    (_, None) => t!("settings-page.looking"),
                };
                let waiting =
                    source == Source::Snap && enabled && cx.snap.is_some_and(crate::snap::State::wants_socket);
                list.row(SettingRow::new(name).description(description), |ui| {
                    if waiting {
                        let start = Button::new(t!("snap.socket-on"))
                            .variant("primary")
                            .loading(cx.planning)
                            .on_press(Msg::SnapSocket(true));
                        ui.add(start).id("snap-socket-on");
                    }
                    ui.add(Switch::new(enabled).on_toggle(move |on| Msg::Source(source, on)));
                });
                if source == Source::Flatpak && enabled && cx.flathub == Some(false) {
                    flathub_row(list, cx.planning);
                }
            }
        }
    }
    let preference = settings::aur_preference(cx.settings);
    let chosen = AUR_HELPERS.iter().position(|(choice, _)| *choice == preference);
    let names = AUR_HELPERS.map(|(_, value)| t!(&format!("settings-page.helper-{value}")));
    let row = SettingRow::new(t!("settings-page.aur-helper")).description(t!("settings-page.aur-helper-text"));
    list.row(row, |ui| {
        ui.add(Select::new(names).selected(chosen).on_select(Msg::AurHelper)).width(Length::Cells(CONTROL_WIDTH));
    });
}

/// What the Snap row says about snapd: not installed, its socket off, not answering, or its
/// version.
fn snap_description(state: &SnapState) -> String {
    match state {
        SnapState::Ready(info) => t!("snap.state.ready", version = info.version.as_str()),
        other => t!(other.key()),
    }
}

/// Flatpak is on but the user has no Flathub remote, so it has nothing to install: an offer to add
/// it, which needs no permission.
fn flathub_row(list: &mut SettingsRows<'_, Msg>, planning: bool) {
    let row = SettingRow::new(t!("flatpak.flathub")).description(t!("flatpak.flathub-missing"));
    list.row(row, |ui| {
        let add = Button::new(t!("flatpak.add")).variant("primary").loading(planning).on_press(Msg::AddFlathub);
        ui.add(add).id("add-flathub");
    });
}

/// A source this machine lacks: an offer to install it where its package is in the
/// repositories, the reason where it is not.
fn missing_row(list: &mut SettingsRows<'_, Msg>, source: Source, name: String, planning: bool) {
    if let (None, Some(package)) = (sources::package(source), sources::aur_package(source)) {
        // Nothing in the repositories brings it, so it is built from the AUR like any other
        // package: the offer says so, because a build takes minutes rather than seconds.
        let row = SettingRow::new(name).description(t!("source.missing-aur", package = package));
        list.row(row, |ui| {
            let build = Button::new(t!("source.build", package = package))
                .variant("primary")
                .loading(planning)
                .on_press(Msg::BuildFromAur(source));
            ui.add(build).id(format!("build-{}", sources::name(source)));
        });
        return;
    }
    match sources::package(source) {
        Some(package) => {
            let row = SettingRow::new(name).description(t!("source.missing-message", package = package));
            list.row(row, |ui| {
                let install = Button::new(t!("source.install", package = package))
                    .variant("primary")
                    .loading(planning)
                    .on_press(Msg::Install(source));
                ui.add(install).id(format!("install-{}", sources::name(source)));
            });
        }
        None => {
            let row = SettingRow::new(name).description(t!("source.missing-later")).disabled(true);
            list.row(row, |ui| {
                ui.add(Text::new(t!("source.missing")).role("faint").no_wrap());
            });
        }
    }
}

/// The program that asks for administrator permission.
fn privilege_section(list: &mut SettingsRows<'_, Msg>, cx: Cx<'_>) {
    list.heading(t!("settings-page.privilege"));
    let tool = settings::privilege_tool(cx.settings);
    let chosen = PRIVILEGE_TOOLS.iter().position(|(choice, _)| *choice == tool);
    let names = PRIVILEGE_TOOLS.map(|(_, value)| t!(&format!("settings-page.tool-{value}")));
    let in_use = match cx.tool {
        Tool::Pkexec(_) => t!("settings-page.tool-uses-pkexec"),
        Tool::Sudo => t!("settings-page.tool-uses-sudo"),
    };
    list.row(SettingRow::new(t!("settings-page.tool")).description(in_use), |ui| {
        ui.add(Select::new(names).selected(chosen).on_select(Msg::PrivilegeTool)).width(Length::Cells(CONTROL_WIDTH));
    });
}

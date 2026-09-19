//! The settings page: every setting qpac reads, one row each under a heading, saved the moment
//! it changes.
//!
//! Each heading is one function that fills its own list, so a new group of settings is a new
//! function and one line in [`view`]. The page holds no state of its own: the values live in the
//! application's settings, the row the keyboard is on lives in each list.

mod backend;
#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod tests;

use qframe::icons::{IconMode, PillarStyle};
use qframe::prelude::*;
use qframe::storage::Settings;
use qframe::widgets::{ScrollView, Select, SettingRow, SettingsList, SettingsRows, Switch};
use qpackages_core::sources::{Availability, Source, Sources};

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
    /// A setting every application of the family shares was chosen.
    Shared(Shared),
    /// A setting of what happens around the packages changed.
    Backend(BackendMsg),
}

/// A change to what the framework keeps for every application of the family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shared {
    /// The language, by code.
    Language(String),
    /// The theme, by id.
    Theme(String),
    /// The icon mode.
    Icons(IconMode),
    /// Reduced motion.
    ReducedMotion(bool),
    /// The pillar style.
    Pillar(PillarStyle),
}

/// What the page needs from the rest of the application to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Cx<'a> {
    /// The settings as they stand.
    pub settings: &'a Settings,
    /// Which sources this machine has, once the first read has said.
    pub sources: Option<&'a Sources>,
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
}

/// Draws the page.
pub fn view(ui: &mut View<'_, Msg>, cx: Cx<'_>) {
    let appearance = Appearance::read(ui.env());
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Button::new(t!("settings-page.back")).icon("arrow-left").on_press(Msg::Back)).id("settings-back");
            ui.add(Text::new(t!("settings-page.title")).role("title").no_wrap());
        })
        .gap(2)
        .fill_width();
        // One list per heading: a scroll view reveals a focused widget whole, and a single list
        // taller than the screen would jump to its end the moment it took focus, moving the row
        // that was clicked out from under the pointer.
        ui.add_with(ScrollView::new(), |ui| {
            ui.column(|ui| {
                SettingsList::show(ui, |list| sources_section(list, cx)).fill_width().id("settings-sources");
                SettingsList::show(ui, |list| backend::updates_section(list, cx)).fill_width().id("settings-updates");
                SettingsList::show(ui, |list| backend::backup_section(list, cx)).fill_width().id("settings-backup");
                SettingsList::show(ui, |list| backend::cleanup_section(list, cx)).fill_width().id("settings-cleanup");
                SettingsList::show(ui, |list| backend::mirrors_section(list, cx)).fill_width().id("settings-mirrors");
                SettingsList::show(ui, |list| privilege_section(list, cx)).fill_width().id("settings-privilege");
                SettingsList::show(ui, |list| appearance_section(list, &appearance))
                    .fill_width()
                    .id("settings-appearance");
            })
            .fill_width();
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
                    (_, Some(_)) => t!("settings-page.installed"),
                    (_, None) => t!("settings-page.looking"),
                };
                list.row(SettingRow::new(name).description(description), |ui| {
                    ui.add(Switch::new(enabled).on_toggle(move |on| Msg::Source(source, on)));
                });
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

/// A source this machine lacks: an offer to install it where its package is in the
/// repositories, the reason where it is not.
fn missing_row(list: &mut SettingsRows<'_, Msg>, source: Source, name: String, planning: bool) {
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

/// What the appearance rows show, read from the environment before the list is built.
struct Appearance {
    languages: Vec<(String, String)>,
    language: String,
    themes: Vec<(String, String)>,
    theme: String,
    icons: IconMode,
    reduced: bool,
    forced: bool,
    pillar: PillarStyle,
}

impl Appearance {
    fn read(env: &qframe::env::Env) -> Self {
        Self {
            languages: env.i18n().list(),
            language: env.i18n().active().to_owned(),
            themes: env.themes(),
            theme: env.theme().id().to_owned(),
            icons: env.icon_mode(),
            reduced: env.reduced_motion(),
            forced: env.reduced_motion_forced(),
            pillar: env.pillar_style().unwrap_or(PillarStyle::Thick),
        }
    }
}

/// The settings every application of the family shares: language, theme, icons, motion and the
/// pillar. A change applies at once and is kept in qpac's own file.
fn appearance_section(list: &mut SettingsRows<'_, Msg>, now: &Appearance) {
    list.heading(t!("settings-page.appearance"));
    let codes: Vec<String> = now.languages.iter().map(|(code, _)| code.clone()).collect();
    let names: Vec<String> = now.languages.iter().map(|(_, name)| name.clone()).collect();
    let chosen = codes.iter().position(|code| *code == now.language);
    list.row(SettingRow::new(t!("settings-page.language")), |ui| {
        let select = Select::new(names)
            .selected(chosen)
            .on_select(move |index| Msg::Shared(Shared::Language(codes[index].clone())));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
    let ids: Vec<String> = now.themes.iter().map(|(id, _)| id.clone()).collect();
    let titles: Vec<String> = now.themes.iter().map(|(_, title)| title.clone()).collect();
    let chosen = ids.iter().position(|id| *id == now.theme);
    list.row(SettingRow::new(t!("settings-page.theme")), |ui| {
        let select =
            Select::new(titles).selected(chosen).on_select(move |index| Msg::Shared(Shared::Theme(ids[index].clone())));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
    let modes = IconMode::ALL.map(|mode| t!(&format!("settings-page.icons-{}", mode.name())));
    let chosen = IconMode::ALL.iter().position(|mode| *mode == now.icons);
    list.row(SettingRow::new(t!("settings-page.icons")), |ui| {
        let select =
            Select::new(modes).selected(chosen).on_select(|index| Msg::Shared(Shared::Icons(IconMode::ALL[index])));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
    let note = match (now.forced, now.reduced) {
        (true, true) => t!("settings-page.forced-on"),
        (true, false) => t!("settings-page.forced-off"),
        (false, _) => t!("settings-page.reduce-motion-text"),
    };
    let row = SettingRow::new(t!("settings-page.reduce-motion")).description(note).disabled(now.forced);
    let (reduced, forced) = (now.reduced, now.forced);
    list.row(row, |ui| {
        ui.add(Switch::new(reduced).disabled(forced).on_toggle(|on| Msg::Shared(Shared::ReducedMotion(on))));
    });
    let styles = PillarStyle::ALL.map(|style| t!(&format!("settings-page.pillar-{}", style.name())));
    let chosen = PillarStyle::ALL.iter().position(|style| *style == now.pillar).unwrap_or(0);
    list.row(SettingRow::new(t!("settings-page.pillar")), |ui| {
        ui.add(
            qframe::widgets::Segmented::new(styles)
                .selected(chosen)
                .on_select(|index| Msg::Shared(Shared::Pillar(PillarStyle::ALL[index]))),
        );
    });
}

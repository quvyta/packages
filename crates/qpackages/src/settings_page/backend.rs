//! The settings of what happens around the packages: the background check, the snapshot before
//! an update, the orphans a change leaves, and pacman's mirrors.
//!
//! Every row is a full control and saves the moment it changes. The two that need the
//! administrator, choosing the mirrors and switching reflector's timer, go through the same
//! confirmation and helper as a transaction.

use qframe::prelude::*;
use qframe::widgets::{Modal, Select, SettingRow, SettingsRows, Switch, Table, TableCell, TableRow};
use qpackages_core::backup::{Setting, Tool};
use qpackages_core::reflector::{Country, Sort};

use super::{CONTROL_WIDTH, Cx, Msg as PageMsg};
use crate::backend_settings::{
    self, OFFERED_AGES, OFFERED_COUNTS, OFFERED_INTERVALS, OFFERED_SORTS, Orphans, backup_choices,
};
use crate::transaction::view::backup_note;

/// Everything that can happen in these settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// The background check was turned on or off.
    Autostart(bool),
    /// The background check was switched, or why not; `true` when it was being turned on.
    AutostartSwitched(bool, Result<(), String>),
    /// An interval was chosen, by position in `OFFERED_INTERVALS`.
    Interval(usize),
    /// A snapshot choice was made, by position among those this machine offers.
    Backup(usize),
    /// What happens to orphans was chosen, by position in `Orphans::ALL`.
    Orphans(usize),
    /// reflector listed its countries, or why not.
    Countries(Result<Vec<qpackages_core::reflector::Country>, String>),
    /// The list of countries was opened.
    ChooseCountries,
    /// The list of countries was closed.
    CountriesDone,
    /// A country was checked or unchecked, by its row in the list.
    ToggleCountry(usize),
    /// A mirror count was chosen, by position in `OFFERED_COUNTS`.
    MirrorCount(usize),
    /// An age was chosen, by position in `OFFERED_AGES`.
    MirrorAge(usize),
    /// An order was chosen, by position in `OFFERED_SORTS`.
    MirrorSort(usize),
    /// The mirrors were asked to be chosen now with these settings.
    ApplyMirrors,
    /// reflector's timer was asked to be switched.
    ReflectorTimer(bool),
    /// The user asked to install reflector.
    InstallReflector,
}

/// What the page knows about reflector on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reflector {
    /// Whether reflector is installed.
    pub installed: bool,
    /// The countries reflector knows mirrors in, as far as they were read.
    pub countries: Countries,
    /// Whether reflector's timer is enabled.
    pub timer_on: bool,
    /// Whether the list of countries is open for choosing.
    pub choosing: bool,
}

/// The countries reflector lists, read without privileges when the page first needs them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Countries {
    /// Not asked yet.
    #[default]
    Unread,
    /// reflector is being asked.
    Reading,
    /// What reflector listed.
    Read(Vec<Country>),
    /// reflector could not list them; what it said.
    Failed(String),
}

impl Countries {
    /// The countries, once read.
    #[must_use]
    pub fn known(&self) -> &[Country] {
        match self {
            Self::Read(countries) => countries,
            _ => &[],
        }
    }
}

/// The background check: on or off, and how often.
pub(super) fn updates_section(list: &mut SettingsRows<'_, PageMsg>, cx: Cx<'_>) {
    list.heading(t!("settings-page.updates"));
    let on = backend_settings::autostart(cx.settings);
    let hours = backend_settings::interval_hours(cx.settings);
    let description = if cx.background { t!("settings-page.check-text") } else { t!("settings-page.check-no-home") };
    let row = SettingRow::new(t!("settings-page.check")).description(description).disabled(!cx.background);
    list.row(row, |ui| {
        ui.add(Switch::new(on).disabled(!cx.background).on_toggle(|on| PageMsg::Backend(Msg::Autostart(on))));
    });
    let names = OFFERED_INTERVALS.map(interval_name);
    let chosen = OFFERED_INTERVALS.iter().position(|offered| *offered == hours);
    let row = SettingRow::new(t!("settings-page.interval")).description(t!("settings-page.interval-text"));
    list.row(row, |ui| {
        let select = Select::new(names)
            .selected(chosen)
            .placeholder(interval_name(hours))
            .on_select(|index| PageMsg::Backend(Msg::Interval(index)));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
}

/// How often, as the interval choice says it.
fn interval_name(hours: u32) -> String {
    match hours {
        24 => t!("settings-page.interval-day"),
        168 => t!("settings-page.interval-week"),
        hours => t!("settings-page.interval-hours", n = hours),
    }
}

/// The snapshot tool: off or one of those installed. A tool that is not installed is a row of
/// its own that says so and cannot be chosen: qpac never sets one up, since that is a decision
/// about the file system.
pub(super) fn backup_section(list: &mut SettingsRows<'_, PageMsg>, cx: Cx<'_>) {
    list.heading(t!("settings-page.backup"));
    let setting = backend_settings::backup_tool(cx.settings, cx.detected);
    let plan = qpackages_core::backup::Plan::new(setting, cx.detected);
    let choices = backup_choices(cx.detected);
    let names: Vec<String> = choices.iter().map(|choice| backup_name(*choice)).collect();
    let chosen = choices.iter().position(|choice| *choice == setting);
    let description = match backup_note(plan) {
        Some(note) => note,
        None if choices.len() == 1 => t!("settings-page.tool-not-installed"),
        None => t!("settings-page.backup-off"),
    };
    list.row(SettingRow::new(t!("settings-page.backup-tool")).description(description), |ui| {
        let select = Select::new(names)
            .selected(chosen)
            .placeholder(backup_name(setting))
            .on_select(|index| PageMsg::Backend(Msg::Backup(index)));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
    for tool in [Tool::Snapper, Tool::Timeshift] {
        if choices.contains(&Setting::Tool(tool)) {
            continue;
        }
        // One line each: why no tool can be chosen is said once, on the choice above.
        let state = if tool == Tool::Snapper && cx.detected.snapper {
            t!("settings-page.snapper-no-root")
        } else {
            t!("source.missing")
        };
        list.row(SettingRow::new(tool.key()).disabled(true), |ui| {
            ui.add(Text::new(state).role("faint").no_wrap());
        });
    }
}

/// A snapshot choice as the list names it.
fn backup_name(setting: Setting) -> String {
    match setting {
        Setting::Off => t!("settings-page.backup-none"),
        Setting::Tool(tool) => tool.key().to_owned(),
    }
}

/// What happens to the orphans a removal or an update leaves.
pub(super) fn cleanup_section(list: &mut SettingsRows<'_, PageMsg>, cx: Cx<'_>) {
    list.heading(t!("settings-page.cleanup"));
    let now = backend_settings::orphans(cx.settings);
    let names = Orphans::ALL.map(|(_, value)| t!(&format!("settings-page.orphans-{value}")));
    let chosen = Orphans::ALL.iter().position(|(choice, _)| *choice == now);
    let row = SettingRow::new(t!("settings-page.orphans")).description(t!("settings-page.orphans-text"));
    list.row(row, |ui| {
        ui.add(Select::new(names).selected(chosen).on_select(|index| PageMsg::Backend(Msg::Orphans(index))))
            .width(Length::Cells(CONTROL_WIDTH));
    });
}

/// pacman's mirrors, chosen by reflector: the countries, how many, how fresh and in what
/// order, the button that applies them, and reflector's weekly timer. Without reflector, one
/// row that offers to install it.
pub(super) fn mirrors_section(list: &mut SettingsRows<'_, PageMsg>, cx: Cx<'_>) {
    list.heading(t!("settings-page.mirrors"));
    let reflector = cx.reflector;
    if !reflector.installed {
        let row = SettingRow::new("reflector").description(t!("settings-page.reflector-missing"));
        list.row(row, |ui| {
            let install = Button::new(t!("source.install", package = "reflector"))
                .variant("primary")
                .loading(cx.planning)
                .on_press(PageMsg::Backend(Msg::InstallReflector));
            ui.add(install).id("install-reflector");
        });
        return;
    }
    let codes = backend_settings::mirror_countries(cx.settings);
    let chosen = match (&reflector.countries, codes.is_empty()) {
        (_, true) => t!("mirrors.every-country"),
        (Countries::Read(known), false) => codes
            .iter()
            .map(|code| known.iter().find(|country| country.code == *code).map_or(code.as_str(), |c| c.name.as_str()))
            .collect::<Vec<_>>()
            .join(", "),
        (_, false) => codes.join(", "),
    };
    let (description, ready) = match &reflector.countries {
        Countries::Read(_) => (chosen, true),
        Countries::Unread | Countries::Reading => (t!("settings-page.countries-reading"), false),
        Countries::Failed(reason) => (t!("settings-page.countries-failed", reason = reason.as_str()), false),
    };
    list.row(SettingRow::new(t!("settings-page.countries")).description(description), |ui| {
        let choose =
            Button::new(t!("settings-page.choose")).disabled(!ready).on_press(PageMsg::Backend(Msg::ChooseCountries));
        ui.add(choose).id("choose-countries");
    });
    let count = backend_settings::mirror_count(cx.settings);
    let names = OFFERED_COUNTS.map(|n| t!("settings-page.count-value", n = u32::from(n)));
    list.row(SettingRow::new(t!("settings-page.count")), |ui| {
        let chosen = OFFERED_COUNTS.iter().position(|offered| *offered == count);
        let select = Select::new(names)
            .selected(chosen)
            .placeholder(t!("settings-page.count-value", n = u32::from(count)))
            .on_select(|index| PageMsg::Backend(Msg::MirrorCount(index)));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
    let age = backend_settings::mirror_age(cx.settings);
    let names = OFFERED_AGES.map(|hours| t!("settings-page.age-value", n = u32::from(hours)));
    list.row(SettingRow::new(t!("settings-page.age")), |ui| {
        let chosen = OFFERED_AGES.iter().position(|offered| *offered == age);
        let select = Select::new(names)
            .selected(chosen)
            .placeholder(t!("settings-page.age-value", n = u32::from(age)))
            .on_select(|index| PageMsg::Backend(Msg::MirrorAge(index)));
        ui.add(select).width(Length::Cells(CONTROL_WIDTH));
    });
    let sort = backend_settings::mirror_sort(cx.settings);
    let names = OFFERED_SORTS.map(sort_name);
    list.row(SettingRow::new(t!("settings-page.sort")), |ui| {
        let chosen = OFFERED_SORTS.iter().position(|offered| *offered == sort);
        ui.add(Select::new(names).selected(chosen).on_select(|index| PageMsg::Backend(Msg::MirrorSort(index))))
            .width(Length::Cells(CONTROL_WIDTH));
    });
    let row = SettingRow::new(t!("settings-page.apply-mirrors")).description(t!("settings-page.apply-mirrors-text"));
    list.row(row, |ui| {
        let apply = Button::new(t!("mirrors.apply"))
            .variant("primary")
            .disabled(!(ready || codes.is_empty()) || cx.busy)
            .loading(cx.planning)
            .on_press(PageMsg::Backend(Msg::ApplyMirrors));
        ui.add(apply).id("apply-mirrors");
    });
    let on = reflector.timer_on;
    let row = SettingRow::new(t!("settings-page.timer")).description(t!("settings-page.timer-text"));
    list.row(row, |ui| {
        ui.add(Switch::new(on).disabled(cx.busy).on_toggle(|on| PageMsg::Backend(Msg::ReflectorTimer(on))));
    });
}

/// An order as the list names it.
fn sort_name(sort: Sort) -> String {
    t!(&format!("mirrors.sort-{}", sort.key()))
}

/// The countries to choose from, over the page: each with its code and how many mirrors it has,
/// the chosen ones checked. A choice is saved as it is made; closing only closes.
pub(super) fn countries(ui: &mut View<'_, PageMsg>, cx: Cx<'_>) {
    if !cx.reflector.choosing {
        return;
    }
    let known = cx.reflector.countries.known();
    let codes = backend_settings::mirror_countries(cx.settings);
    let columns = [
        qframe::widgets::Column::new(t!("settings-page.country")).min(12),
        qframe::widgets::Column::new(t!("settings-page.code")).width(qframe::widgets::ColumnWidth::Fixed(6)),
        qframe::widgets::Column::new(t!("settings-page.mirror-count"))
            .width(qframe::widgets::ColumnWidth::Fixed(8))
            .align(Align::End),
    ];
    let rows: Vec<TableRow> = known
        .iter()
        .map(|country| {
            TableRow::new([
                TableCell::new(country.name.clone()),
                TableCell::new(country.code.clone()),
                TableCell::new(country.mirrors.to_string()),
            ])
        })
        .collect();
    let checked: Vec<bool> = known.iter().map(|country| codes.contains(&country.code)).collect();
    let dialog =
        Modal::new().title(t!("settings-page.countries-title")).on_close(PageMsg::Backend(Msg::CountriesDone)).action(
            Button::new(t!("settings-page.done")).variant("primary").on_press(PageMsg::Backend(Msg::CountriesDone)),
        );
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(t!("settings-page.countries-lead")).role("secondary")).fill_width();
        let height = u16::try_from(known.len()).unwrap_or(u16::MAX).saturating_add(1).clamp(2, 14);
        let table =
            Table::new(columns, rows).checked(checked).on_toggle(|row| PageMsg::Backend(Msg::ToggleCountry(row)));
        ui.add(table).fill_width().height(Length::Cells(height)).id("countries");
    });
}

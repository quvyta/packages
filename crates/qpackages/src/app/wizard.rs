//! The first start: the framework's setup wizard, with qpac's own two steps, the sources and the
//! background check.
//!
//! It opens while qpac has no `packages.conf` of its own, and only then: someone who has used an
//! earlier release keeps their file (brought over from the old folder first) and never sees it.
//! The framework owns the appearance step, the buttons and the two files; nothing at all is
//! written until Finish, so a qpac closed half-way leaves the settings folder exactly as it was and
//! the wizard comes again next start.
//!
//! The sources step is the four sources as checkboxes, those this machine has checked. One it
//! lacks is offered: checked, it is brought after the wizard through the same confirmation the
//! Settings page's install or build button opens, so nothing is installed that the person did not
//! agree to in the normal dialog. The background step asks whether the user timer looks for
//! updates, Yes by default (the design's "on by default" is a question whose answer starts as
//! Yes), and how often.

use std::path::{Path, PathBuf};

use qframe::prelude::*;
use qframe::storage::{Family, Preferences, Settings};
use qframe::widgets::{Appearance, Checkbox, ScrollView, Select, SettingRow, SettingsList, Setup, SetupWizard, Switch};
use qpackages_core::sources::{Availability, Source};

use super::{Msg, Qpackages, store_sources};
use crate::backend_settings::{self, DEFAULT_INTERVAL, OFFERED_INTERVALS};
use crate::settings_page::{self, CONTROL_WIDTH, interval_name};
use crate::{settings, sources};

/// The widget id the keyboard starts on: the appearance rows of the framework's step.
pub(super) const FIRST: &str = "setup-appearance";

/// Rows the wizard takes around its page: the padding, the name, the blank line under it, the
/// steps, the blank lines around the page and the row of buttons.
const AROUND_PAGE: u16 = 8;

/// The fewest rows the page keeps, however short the terminal is.
const LEAST_PAGE_ROWS: u16 = 8;

/// Cells a source's line keeps from the page's left edge, so it stands under its name rather
/// than under its checkbox.
const LINE_INDENT: u16 = 4;

/// The wizard qpac opens on its first start, and the family folder it writes into.
pub struct FirstRun {
    pub(super) setup: Setup<Msg>,
    pub(super) folder: PathBuf,
}

impl FirstRun {
    /// The wizard over the family folder `folder` while qpac has no `packages.conf` there; `None`
    /// once it has one, and then the wizard never opens.
    #[must_use]
    pub fn in_folder(folder: &Path) -> Option<Self> {
        let setup =
            Setup::new_in(folder, Family::QUVYTA, settings::APP, &crate::i18n(), Msg::Setup).on_finish(Msg::SetUp);
        setup.needed().then(|| Self { setup, folder: folder.to_path_buf() })
    }

    /// The shared preferences as the wizard resolved them, without writing anything: what the
    /// screen starts with while it asks.
    #[must_use]
    pub fn preferences(&self) -> &Preferences {
        self.setup.preferences()
    }

    /// The same wizard looking for a Nerd Font only in `fonts`, and installing one there without
    /// registering it, so a test never looks at or changes the user's own fonts.
    #[cfg(test)]
    pub(crate) fn with_fonts(mut self, fonts: &Path) -> Self {
        let install = qframe::icons::nerd_font::Install::new().target(fonts.join("QuvytaNerdFont")).register(false);
        self.setup = self.setup.install(install).font_dirs(vec![fonts.to_path_buf()]);
        self
    }
}

/// Something on one of qpac's own steps of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardMsg {
    /// A source was checked or unchecked.
    Pick(Source, bool),
    /// The background check was turned on or off.
    Background(bool),
    /// An interval was chosen, by position in `OFFERED_INTERVALS`.
    Interval(usize),
}

/// What qpac's own steps hold until Finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Choices {
    /// Each source of [`sources::ALL`] as the person checked it; `None` where it still follows
    /// what the machine has.
    picked: [Option<bool>; 4],
    /// Whether the background check is to be turned on.
    background: bool,
    /// Hours between background checks.
    interval: u32,
}

impl Default for Choices {
    fn default() -> Self {
        Self { picked: [None; 4], background: true, interval: DEFAULT_INTERVAL }
    }
}

/// Where `source` stands in [`sources::ALL`].
fn place(source: Source) -> usize {
    sources::ALL.iter().position(|known| *known == source).unwrap_or_default()
}

impl Qpackages {
    /// Whether the first-run wizard has the screen.
    pub(super) fn setting_up(&self) -> bool {
        self.setup.as_ref().is_some_and(Setup::needed)
    }

    /// What the first read found about `source`; `None` before it answered.
    fn found(&self, source: Source) -> Option<&Availability> {
        self.library.as_ref().map(|found| found.sources.get(source))
    }

    /// Whether `source` can be checked and unchecked on the wizard.
    ///
    /// pacman cannot: it is the system's own package manager, the settings have no switch for it
    /// either, and without it there is nothing to manage. Nothing can before the first read, which
    /// is what says what the machine has. The AUR cannot as root, who never builds its packages,
    /// and a missing Snap only while the AUR is checked, since snapd is built from there.
    pub(super) fn choosable(&self, source: Source) -> bool {
        let Some(found) = self.found(source) else { return false };
        match source {
            Source::Pacman => false,
            Source::Aur => !self.root,
            Source::Snap if matches!(found, Availability::Missing) => !self.root && self.checked(Source::Aur),
            Source::Flatpak | Source::Snap => true,
        }
    }

    /// Whether `source` is checked on the wizard: what the person chose, else whether the
    /// machine has it. One that cannot be chosen is unchecked, apart from pacman, which is
    /// always on.
    pub(super) fn checked(&self, source: Source) -> bool {
        if source == Source::Pacman {
            return true;
        }
        if !self.choosable(source) {
            return false;
        }
        let detected = matches!(self.found(source), Some(Availability::Ready { .. }));
        self.choices.picked[place(source)].unwrap_or(detected)
    }

    /// Whether the background check can be switched: there is a unit folder and qpac's own path.
    fn background_possible(&self) -> bool {
        self.places.units.is_some() && self.places.exe.is_some()
    }

    pub(super) fn update_wizard(&mut self, msg: WizardMsg) {
        match msg {
            WizardMsg::Pick(source, on) => {
                if self.choosable(source) {
                    self.choices.picked[place(source)] = Some(on);
                }
            }
            WizardMsg::Background(on) => self.choices.background = on,
            WizardMsg::Interval(index) => {
                if let Some(&hours) = OFFERED_INTERVALS.get(index) {
                    self.choices.interval = hours;
                }
            }
        }
    }

    /// The wizard wrote the shared keys and made `packages.conf`: qpac's own settings go in
    /// beside them, the background check is switched the way the Settings page switches it, the
    /// Settings page carries on from the look that was chosen, and the sources that were checked
    /// but are missing start on their way through the normal confirmation.
    pub(super) fn finish_setup(&mut self) -> Command<Msg> {
        let Some(setup) = self.setup.take() else { return Command::none() };
        // The appearance rows of the Settings page start from what the wizard chose; the wizard's
        // own Appearance held those values without writing them.
        let appearance = Appearance::new(Family::QUVYTA, settings::APP, setup.preferences().clone());
        self.appearance = match &self.setup_folder {
            Some(folder) => appearance.in_folder(folder),
            None => appearance,
        };
        let mut wanted = Vec::new();
        // Before the first read nothing was shown to choose from, so the sources stay as they are.
        if self.library.is_some() {
            for source in [Source::Aur, Source::Flatpak, Source::Snap] {
                // As root the AUR was not on offer; the file keeps whatever the user decides when
                // qpac runs as themselves.
                if source == Source::Aur && self.root {
                    continue;
                }
                let on = self.checked(source);
                // A value that is the default is taken out rather than written, so the file holds
                // only what was chosen.
                store(&mut self.settings, &format!("sources.{}", sources::name(source)), on, on);
                if on && matches!(self.found(source), Some(Availability::Missing)) {
                    wanted.extend(bring(source));
                }
            }
            self.discover.set_enabled(store_sources(&self.settings, self.root));
            self.update_builder();
        }
        let hours = self.choices.interval;
        store(&mut self.settings, backend_settings::INTERVAL, i64::from(hours), hours == DEFAULT_INTERVAL);
        // The timer is switched on the way the Settings page switches it, which saves too; turned
        // down, there is nothing to switch off, since nothing was ever written.
        let timer = if self.choices.background && self.background_possible() {
            self.switch_background(true)
        } else {
            self.settings.remove(backend_settings::AUTOSTART);
            Command::none()
        };
        self.after_setup = wanted.into();
        let next = self.next_after_setup();
        Command::batch([self.save(), timer, next])
    }

    /// Starts what the Settings page would for the next source the wizard was asked to bring,
    /// once the flow is at rest and the packages were read again after the one before: a paru
    /// that was just installed is what builds snapd. One that is on this machine by then is
    /// passed over, and so are snapd's service and link when snapd did not come.
    pub(super) fn next_after_setup(&mut self) -> Command<Msg> {
        // Taken out while it is worked on, so the flow's own messages sent from here do not start
        // the one after it as well.
        let mut queue = std::mem::take(&mut self.after_setup);
        let mut commands = Vec::new();
        while !self.setting_up() && self.transaction.at_rest() && !self.transaction.reading_again() {
            let Some(found) = self.library.as_ref() else { break };
            let Some(msg) = queue.pop_front() else { break };
            let due = match &msg {
                settings_page::Msg::Install(source) | settings_page::Msg::BuildFromAur(source) => {
                    matches!(found.sources.get(*source), Availability::Missing)
                }
                settings_page::Msg::SnapSocket(_) => {
                    matches!(found.sources.get(Source::Snap), Availability::Ready { .. })
                }
                _ => true,
            };
            if due {
                // A plan the flow refused at once, such as a build with no helper, has said why
                // and left the flow at rest: the next one goes on.
                commands.push(self.settings_message(msg));
            }
        }
        queue.append(&mut self.after_setup);
        self.after_setup = queue;
        Command::batch(commands)
    }

    /// The wizard, while it is wanted: the framework's appearance step, then qpac's own two.
    pub(super) fn setup_wizard(&self, ui: &mut View<'_, Msg>) {
        let Some(setup) = &self.setup else { return };
        let rows = ui.size().height.saturating_sub(AROUND_PAGE).max(LEAST_PAGE_ROWS);
        ui.column(|ui| {
            ui.add(Text::new("qpac").color("accent").bold().no_wrap()).fill_width();
            ui.spacer().height(Length::Cells(1));
            // Every step is given the rows that are left, so the buttons stand at the bottom
            // wherever the person is and a page longer than the screen scrolls instead of pushing
            // them off it.
            SetupWizard::new(setup)
                .step(t!("wizard.step-sources"), |ui| self.sources_step(ui))
                .step(t!("wizard.step-updates"), |ui| self.updates_step(ui))
                .page_height(rows)
                .show(ui)
                .fill_width();
        })
        .padding(Padding::symmetric(1, 2))
        .fill();
    }

    /// The sources, each with its checkbox and one line saying what checking it means.
    fn sources_step(&self, ui: &mut View<'_, Msg>) {
        ui.add(Text::new(t!("wizard.sources-intro")).role("secondary")).fill_width();
        ui.spacer().height(Length::Cells(1));
        // Four sources with their lines outgrow a short screen; the page scrolls rather than
        // cutting the last of them off.
        ui.add_with(ScrollView::new(), |ui| {
            for source in sources::ALL {
                self.source_row(source, ui);
            }
        })
        .fill()
        .id("wizard-sources");
    }

    fn source_row(&self, source: Source, ui: &mut View<'_, Msg>) {
        let name = sources::name(source);
        let choosable = self.choosable(source);
        let checkbox = Checkbox::new(self.checked(source))
            .label(t!(&format!("source.{name}")))
            .disabled(!choosable)
            .on_toggle(move |on| Msg::Wizard(WizardMsg::Pick(source, on)));
        ui.add(checkbox).id(format!("wizard-{name}"));
        let (line, faint) = self.source_line(source);
        let role = if faint { "faint" } else { "secondary" };
        ui.add(Text::new(line).role(role)).padding(Padding { left: LINE_INDENT, ..Padding::default() }).fill_width();
    }

    /// What `source`'s line says, and whether it is faint: it is when the source cannot be
    /// checked for a reason the line gives.
    fn source_line(&self, source: Source) -> (String, bool) {
        if source == Source::Pacman {
            return (t!("wizard.pacman"), false);
        }
        let Some(found) = self.found(source) else { return (t!("wizard.looking"), false) };
        if source == Source::Aur && self.root {
            return (t!("wizard.aur-root"), true);
        }
        if let Availability::Ready { .. } = found {
            let helper = self.library.as_ref().and_then(|found| found.sources.aur_helper);
            return match (source, helper) {
                (Source::Aur, Some(helper)) => (t!("wizard.found-through", helper = helper.program()), false),
                _ => (t!("wizard.found"), false),
            };
        }
        match (sources::package(source), sources::aur_package(source)) {
            (Some(package), _) => (t!("wizard.missing-install", package = package), false),
            (None, Some(_)) if self.root => (t!("wizard.snap-root"), true),
            (None, Some(_)) if !self.checked(Source::Aur) => (t!("wizard.snap-needs-aur"), true),
            (None, Some(package)) => (t!("wizard.missing-snap", package = package), false),
            (None, None) => (t!("source.missing"), true),
        }
    }

    /// The background check: the same two rows the Settings page has, answered here before
    /// anything is switched.
    fn updates_step(&self, ui: &mut View<'_, Msg>) {
        ui.add(Text::new(t!("wizard.updates-intro")).role("secondary")).fill_width();
        ui.spacer().height(Length::Cells(1));
        let possible = self.background_possible();
        let on = self.choices.background && possible;
        let hours = self.choices.interval;
        ui.add_with(ScrollView::new(), |ui| {
            SettingsList::show(ui, |list| {
                let mut row = SettingRow::new(t!("settings-page.check")).disabled(!possible);
                if !possible {
                    row = row.description(t!("settings-page.check-no-home"));
                }
                list.row(row, |ui| {
                    let switch =
                        Switch::new(on).disabled(!possible).on_toggle(|on| Msg::Wizard(WizardMsg::Background(on)));
                    ui.add(switch).id("wizard-background");
                });
                let names = OFFERED_INTERVALS.map(interval_name);
                let chosen = OFFERED_INTERVALS.iter().position(|offered| *offered == hours);
                let row = SettingRow::new(t!("settings-page.interval")).description(t!("settings-page.interval-text"));
                list.row(row, |ui| {
                    let select =
                        Select::new(names).selected(chosen).on_select(|index| Msg::Wizard(WizardMsg::Interval(index)));
                    ui.add(select).width(Length::Cells(CONTROL_WIDTH)).id("wizard-interval");
                });
            })
            .fill_width()
            .id("wizard-updates");
        })
        .fill();
    }
}

/// What the Settings page starts to bring `source` to this machine: its package installed from
/// the repositories, or built from the AUR, and for snapd its service and `/snap` link after.
fn bring(source: Source) -> Vec<settings_page::Msg> {
    match (sources::package(source), sources::aur_package(source)) {
        (Some(_), _) => vec![settings_page::Msg::Install(source)],
        (None, Some(_)) if source == Source::Snap => {
            vec![settings_page::Msg::BuildFromAur(source), settings_page::Msg::SnapSocket(true)]
        }
        (None, Some(_)) => vec![settings_page::Msg::BuildFromAur(source)],
        (None, None) => Vec::new(),
    }
}

/// Stores `value` under `key`, or takes the key out when the value `is_default`.
fn store<T: qframe::storage::Setting>(settings: &mut Settings, key: &str, value: T, is_default: bool) {
    if is_default {
        settings.remove(key);
    } else {
        settings.set(key, value);
    }
}

#[cfg(test)]
#[path = "wizard_tests.rs"]
mod tests;

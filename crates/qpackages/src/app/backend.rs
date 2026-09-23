//! What the application does for the settings around the packages: saves each choice, switches
//! the background check's user timer, reads reflector's countries, and hands the mirror choice
//! and reflector's timer to the transaction flow, which asks for the administrator.

use std::path::Path;
use std::sync::Arc;

use qframe::prelude::*;
use qframe::widgets::Toast;
use qpackages_core::reflector::{list_countries_args, parse_countries};

use super::{Msg, Qpackages};
use crate::autostart;
use crate::backend_settings::{
    self, OFFERED_AGES, OFFERED_COUNTS, OFFERED_INTERVALS, OFFERED_SORTS, Orphans, backup_choices,
};
use crate::runner::Runner;
use crate::settings_page::{self, BackendMsg, Countries};
use crate::transaction::{self, Action};

/// reflector, found on the search path like the other programs qpac runs as the user.
const REFLECTOR: &str = "reflector";

/// Where systemd keeps the link that enables reflector's timer, from the root of the file
/// system: every user can see it, so the timer's state is read without asking anyone.
const TIMER_LINK: &str = "etc/systemd/system/timers.target.wants/reflector.timer";

impl Qpackages {
    /// Does what a setting of the background check, the snapshots, the orphans or the mirrors
    /// asked.
    pub(super) fn backend_setting(&mut self, msg: BackendMsg) -> Command<Msg> {
        use BackendMsg as Page;
        match msg {
            Page::Autostart(on) => self.switch_background(on),
            Page::AutostartSwitched(on, result) => self.background_switched(on, result),
            Page::Interval(index) => {
                let Some(&hours) = OFFERED_INTERVALS.get(index) else { return Command::none() };
                let changed = backend_settings::set_interval(&mut self.settings, hours);
                // A running timer is written again with the new interval.
                let rewrite = if changed && backend_settings::autostart(&self.settings) {
                    self.switch_background(true)
                } else {
                    Command::none()
                };
                Command::batch([self.saved_if(changed), rewrite])
            }
            Page::Backup(index) => {
                let Some(&choice) = backup_choices(&self.detected).get(index) else { return Command::none() };
                let changed = backend_settings::set_backup_tool(&mut self.settings, choice);
                self.transaction.set_backup(self.backup_plan());
                self.saved_if(changed)
            }
            Page::Orphans(index) => {
                let Some(&(choice, _)) = Orphans::ALL.get(index) else { return Command::none() };
                let changed = backend_settings::set_orphans(&mut self.settings, choice);
                self.saved_if(changed)
            }
            Page::Countries(result) => {
                self.reflector.countries = match result {
                    Ok(countries) => Countries::Read(countries),
                    Err(reason) => Countries::Failed(reason),
                };
                Command::none()
            }
            Page::ChooseCountries => {
                self.reflector.choosing = matches!(self.reflector.countries, Countries::Read(_));
                Command::none()
            }
            Page::CountriesDone => {
                self.reflector.choosing = false;
                Command::none()
            }
            Page::ToggleCountry(row) => {
                let Some(country) = self.reflector.countries.known().get(row) else { return Command::none() };
                let code = country.code.clone();
                let changed = backend_settings::toggle_mirror_country(&mut self.settings, &code);
                self.saved_if(changed)
            }
            Page::MirrorCount(index) => {
                let Some(&count) = OFFERED_COUNTS.get(index) else { return Command::none() };
                let changed = backend_settings::set_mirror_count(&mut self.settings, count);
                self.saved_if(changed)
            }
            Page::MirrorAge(index) => {
                let Some(&hours) = OFFERED_AGES.get(index) else { return Command::none() };
                let changed = backend_settings::set_mirror_age(&mut self.settings, hours);
                self.saved_if(changed)
            }
            Page::MirrorSort(index) => {
                let Some(&sort) = OFFERED_SORTS.get(index) else { return Command::none() };
                let changed = backend_settings::set_mirror_sort(&mut self.settings, sort);
                self.saved_if(changed)
            }
            Page::ApplyMirrors => match backend_settings::mirrors(&self.settings, self.reflector.countries.known()) {
                Ok(mirrors) => self.update(Msg::Transaction(transaction::Msg::Begin(Action::Mirrors(mirrors)))),
                Err(error) => {
                    Command::toast(Toast::warning(t!("settings-page.mirrors-invalid")).body(error.to_string()))
                }
            },
            Page::ReflectorTimer(on) => self.update(Msg::Transaction(transaction::Msg::Begin(Action::Timer(on)))),
            Page::InstallReflector => {
                let install = Action::Install(vec![REFLECTOR.to_owned()]);
                self.update(Msg::Transaction(transaction::Msg::Begin(install)))
            }
        }
    }

    /// Turns the background check's user timer on or off in the background, after saving the
    /// choice; a switch that fails puts the choice back and says why.
    pub(super) fn switch_background(&mut self, on: bool) -> Command<Msg> {
        let (Some(dir), Some(exe)) = (self.places.units.clone(), self.places.exe.clone()) else {
            return Command::none();
        };
        let changed = backend_settings::set_autostart(&mut self.settings, on);
        let hours = backend_settings::interval_hours(&self.settings);
        let runner = Arc::clone(&self.runner);
        let switch = Command::perform(move || {
            let result = if on {
                autostart::switch_on(&dir, &exe, hours, runner.as_ref())
            } else {
                autostart::switch_off(&dir, runner.as_ref())
            };
            Msg::Settings(settings_page::Msg::Backend(BackendMsg::AutostartSwitched(
                on,
                result.map_err(|error| error.to_string()),
            )))
        });
        Command::batch([self.saved_if(changed), switch])
    }

    /// Puts the choice back when the timer could not be switched.
    fn background_switched(&mut self, on: bool, result: Result<(), String>) -> Command<Msg> {
        let Err(reason) = result else {
            return Command::none();
        };
        let changed = backend_settings::set_autostart(&mut self.settings, !on);
        let title = if on { t!("settings-page.check-not-on") } else { t!("settings-page.check-not-off") };
        Command::batch([self.saved_if(changed), Command::toast(Toast::warning(title).body(reason))])
    }

    /// Looks again at what the settings page shows about reflector, and reads its countries the
    /// first time they are needed.
    pub(super) fn look_at_reflector(&mut self) -> Command<Msg> {
        self.reflector.installed = (self.lookup)(REFLECTOR).is_some();
        self.reflector.timer_on = timer_enabled(&self.places.root);
        if !self.reflector.installed || self.reflector.countries != Countries::Unread {
            return Command::none();
        }
        self.reflector.countries = Countries::Reading;
        let runner = Arc::clone(&self.runner);
        Command::perform(move || {
            Msg::Settings(settings_page::Msg::Backend(BackendMsg::Countries(read_countries(runner.as_ref()))))
        })
    }
}

/// Whether reflector's timer is enabled on the machine whose file system starts at `root`.
pub(super) fn timer_enabled(root: &Path) -> bool {
    root.join(TIMER_LINK).exists()
}

/// The countries reflector lists, asked without privileges; what it said when it could not.
fn read_countries(runner: &dyn Runner) -> Result<Vec<qpackages_core::reflector::Country>, String> {
    let output = runner.output(REFLECTOR, &list_countries_args(), &[("LC_ALL", "C")]).map_err(|e| e.to_string())?;
    if !output.succeeded() {
        let said = output.stderr.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
        return Err(said.to_owned());
    }
    Ok(parse_countries(&output.stdout))
}

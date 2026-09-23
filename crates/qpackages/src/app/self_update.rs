//! Saying when a newer qpac is out: the family's once-a-day question to crates.io about qpac
//! itself.
//!
//! This is not the Updates tab. That one is about the packages on this computer and asks the
//! mirrors, the AUR and snapd; this one asks about the program the person is running and nothing
//! else. The two share the word "update", so everything this module shows names qpac and says it
//! is not about the packages.

use std::path::PathBuf;

use qframe::prelude::*;
use qframe::runtime::{Update, UpdateCheck};
use qframe::storage::Family;
use qframe::widgets::Toast;

use super::{Msg, Qpackages};
use crate::settings;

/// How long the notice stays, as long as the family's own: it carries a command to read.
const NOTICE_SECONDS: u64 = 12;

/// Where the family's switch is kept and where qpac remembers when it last asked.
///
/// The switch is the family's, one for every Quvyta application, so it is read from the family's
/// folder rather than from `packages.conf`. A test gives folders of its own, so nothing it does
/// reads or turns off the person's own switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdateFolders {
    /// The family's configuration folder, whose shared file holds the switch.
    pub config: PathBuf,
    /// qpac's state folder, which remembers when the question was last asked.
    pub state: PathBuf,
}

impl SelfUpdateFolders {
    /// This machine's folders, or `None` without a home folder, where nothing could remember the
    /// switch or the last question and so nothing is asked.
    #[must_use]
    pub fn here() -> Option<Self> {
        let family = Family::QUVYTA;
        family.config_dir().zip(family.state_dir(settings::APP)).map(|(config, state)| Self { config, state })
    }
}

/// The folders and the switch as the settings page shows it.
#[derive(Debug, Clone)]
pub(super) struct SelfUpdate {
    folders: SelfUpdateFolders,
    on: bool,
}

impl SelfUpdate {
    /// Whether the switch is on, for the settings page.
    pub(super) fn on(&self) -> bool {
        self.on
    }
}

impl Qpackages {
    /// The same application, asking at start whether a newer qpac is out while the family's
    /// switch in `folders` is on, and showing that switch in the settings. `None` asks nothing
    /// and shows no switch, which is every test that has not said otherwise.
    #[must_use]
    pub fn with_self_update(mut self, folders: Option<SelfUpdateFolders>) -> Self {
        self.self_update = folders.map(|folders| {
            let on = Family::QUVYTA.update_notice_in(&folders.config);
            SelfUpdate { folders, on }
        });
        self
    }

    /// The question for a newer qpac, when the family's switch is on.
    ///
    /// The switch is read from the file here rather than taken from what the page shows: another
    /// Quvyta application may have turned it off since, and a family that turned it off asks
    /// nothing at all.
    pub(super) fn ask_for_newer_qpac(&self) -> Command<Msg> {
        let Some(SelfUpdate { folders, .. }) = &self.self_update else { return Command::none() };
        if !Family::QUVYTA.update_notice_in(&folders.config) {
            return Command::none();
        }
        let check = UpdateCheck::new(
            Family::QUVYTA,
            settings::APP,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            Msg::NewerQpac,
        )
        .in_folders(folders.config.clone(), folders.state.clone());
        Command::check_for_update(check)
    }

    /// Turns the family's switch on or off in its shared file, off the render path.
    pub(super) fn switch_self_update(&mut self, on: bool) -> Command<Msg> {
        let Some(self_update) = &mut self.self_update else { return Command::none() };
        self_update.on = on;
        let folder = self_update.folders.config.clone();
        Command::perform(move || {
            Msg::SelfUpdateSaved(Family::QUVYTA.set_update_notice_in(&folder, on).map_err(|error| error.to_string()))
        })
    }

    /// Says the switch could not be saved, and shows it as the file still has it.
    pub(super) fn self_update_not_saved(&mut self, reason: String) -> Command<Msg> {
        if let Some(self_update) = &mut self.self_update {
            self_update.on = Family::QUVYTA.update_notice_in(&self_update.folders.config);
        }
        Command::toast(Toast::warning(t!("settings-page.not-saved")).body(reason))
    }
}

/// The notice for a newer qpac.
///
/// The family's own notice names the package, `quvyta-packages … is out`, which in a package
/// manager reads like news about packages. This one names qpac and says outright that it is not
/// about the packages on this computer; how to update is the family's: from the launcher, or with
/// `cargo install`.
pub(super) fn notice(update: &Update) -> Toast<Msg> {
    let title = t!("self-update.notice", latest = update.latest());
    let body = t!(
        "self-update.notice-body",
        current = update.current(),
        launcher = Family::QUVYTA.id(),
        package = update.package()
    );
    Toast::info(title).body(body).key("quvyta-update").duration(std::time::Duration::from_secs(NOTICE_SECONDS))
}

#[cfg(test)]
#[path = "self_update_tests.rs"]
mod tests;

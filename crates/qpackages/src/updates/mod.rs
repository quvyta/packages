//! The Updates tab: what has a newer version, grouped by where it comes from, and when that was
//! last looked at.
//!
//! The check itself is [`check::Checker`]; it runs in the background when the application has
//! found its sources and whenever the user asks, and the list already on screen stays while it
//! runs. Installing the updates is not offered yet: the administrator helper has no request that
//! upgrades the system, and the button waits for it.

pub mod check;
mod restart;
#[cfg(test)]
mod tests;

use qframe::prelude::*;
use qframe::widgets::EmptyState;
use qpackages_core::pacman::Update;

use crate::installed::TEXT_INDENT;

use check::{Failure, Found};
use restart::needs_restart;

/// Cells of a list row that are not its label or its note: the pillar with its space, the gap
/// before the note, and the spare cell at the right edge.
const ROW_CHROME: usize = 5;

/// The tab's state.
#[derive(Debug, Default)]
pub struct Updates {
    /// Whether a check is running.
    checking: bool,
    /// When the repositories were last checked to the end, as a Unix timestamp.
    checked_at: Option<i64>,
    /// The repositories' updates, as the last check that reached them found them.
    repo: Option<Vec<Update>>,
    /// Why the latest check could not reach the repositories.
    repo_failure: Option<Failure>,
    /// The AUR's updates, as the last check that asked it found them; `None` when it was not asked.
    aur: Option<Vec<Update>>,
    /// Why the latest check could not reach the AUR.
    aur_failure: Option<Failure>,
    /// The row selected in the list.
    selected: Option<usize>,
}

/// Everything that can happen in the tab.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The user asked for a check.
    CheckNow,
    /// A check ended.
    Checked(Found),
    /// A row of the list was selected.
    Select(usize),
}

/// What the tab asks of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Start a check in the background.
    Check,
}

/// What the tab needs from the rest of the application to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Cx {
    /// Whether the AUR is turned on, so its group is shown.
    pub aur: bool,
    /// The machine's distance from UTC in minutes, for the time of the last check.
    pub utc_offset: i16,
    /// Whether a check can start: not before the sources are known.
    pub can_check: bool,
}

impl Updates {
    /// Applies `msg` and says what the application should do, if anything.
    pub fn update(&mut self, msg: Msg) -> Option<Request> {
        match msg {
            Msg::CheckNow => return self.start(),
            Msg::Checked(found) => self.found(found),
            Msg::Select(row) => self.selected = Some(row),
        }
        None
    }

    /// Marks a check as running and asks for it, unless one already runs.
    pub fn start(&mut self) -> Option<Request> {
        if self.checking {
            return None;
        }
        self.checking = true;
        Some(Request::Check)
    }

    /// How many updates wait, the held-back ones not counted: the number on the tab.
    #[must_use]
    pub fn pending(&self, aur: bool) -> usize {
        let count = |list: &Option<Vec<Update>>| list.iter().flatten().filter(|update| !update.ignored).count();
        count(&self.repo) + if aur { count(&self.aur) } else { 0 }
    }

    /// Takes in what a check found. A part that could not be reached keeps the list it had, so a
    /// check that fails halfway never empties the screen.
    fn found(&mut self, found: Found) {
        self.checking = false;
        match found.repo {
            Ok(updates) => {
                self.repo = Some(updates);
                self.repo_failure = None;
                self.checked_at = Some(found.at);
            }
            Err(failure) => self.repo_failure = Some(failure),
        }
        match found.aur {
            None => {
                self.aur = None;
                self.aur_failure = None;
            }
            Some(Ok(updates)) => {
                self.aur = Some(updates);
                self.aur_failure = None;
            }
            Some(Err(failure)) => self.aur_failure = Some(failure),
        }
    }

    /// Draws the tab.
    pub fn view(&self, ui: &mut View<'_, Msg>, cx: Cx) {
        let time = self.checked_at.map(|at| clock(at, cx.utc_offset));
        let aur = if cx.aur { self.aur.as_deref() } else { None };
        let aur_failure = self.aur_failure.as_ref().filter(|_| cx.aur);
        let known = self.repo.is_some() || aur.is_some();
        let empty = self.repo.as_ref().is_none_or(Vec::is_empty) && aur.is_none_or(<[Update]>::is_empty);
        let check =
            Button::new(t!("updates.check-now")).loading(self.checking).disabled(!cx.can_check).on_press(Msg::CheckNow);

        if !known {
            let state = match (&self.repo_failure, self.checking) {
                (Some(failure), false) => {
                    EmptyState::new(t!("updates.failed")).icon("warning").message(reason(failure)).action(check)
                }
                _ => EmptyState::new(t!("updates.checking")).icon("info").message(t!("updates.checking-message")),
            };
            ui.add(state).fill().id("updates");
            return;
        }
        if empty && self.repo_failure.is_none() && aur_failure.is_none() {
            let message = time.map_or_else(String::new, |time| t!("updates.last-checked", time = time));
            let state = EmptyState::new(t!("updates.up-to-date")).icon("check").message(message).action(check);
            ui.add(state).fill().id("updates");
            return;
        }

        ui.column(|ui| {
            ui.row(|ui| {
                let mut status = vec![t!("updates.count", n = self.pending(cx.aur))];
                if let Some(time) = &time {
                    status.push(t!("updates.last-checked-short", time = time.clone()));
                }
                if self.checking {
                    status.push(t!("updates.checking-short"));
                }
                ui.add(Text::new(status.join(" · ")).no_wrap()).fill_width();
                ui.add(check).id("check-now");
            })
            .gap(2)
            .padding(Padding::symmetric(0, TEXT_INDENT))
            .fill_width();
            if let Some(failure) = &self.repo_failure {
                ui.add(Text::new(t!("updates.stale", reason = reason(failure))).role("faint"))
                    .fill_width()
                    .padding(Padding::symmetric(0, TEXT_INDENT));
            }
            let arrow = ui.env().icons().glyph("arrow-right").into_owned();
            let items = self.items(self.repo.as_deref(), aur, aur_failure, &arrow, usize::from(ui.size().width));
            let list = List::new(items).selected(self.selected).empty_text(t!("updates.none")).on_select(Msg::Select);
            ui.add(list).fill().id("updates");
        })
        .gap(1)
        .fill();
    }

    /// The list's rows: each source's updates under its heading, a source without updates left
    /// out, and a note where a source could not be asked.
    fn items(
        &self,
        repo: Option<&[Update]>,
        aur: Option<&[Update]>,
        aur_failure: Option<&Failure>,
        arrow: &str,
        width: usize,
    ) -> Vec<ListItem> {
        let every = || repo.into_iter().flatten().chain(aur.into_iter().flatten());
        let widest = |text: fn(&Update) -> &str| every().map(|update| text(update).chars().count()).max().unwrap_or(0);
        let widths = (widest(|update| &update.name), widest(|update| &update.from));
        let note = every().filter_map(note).map(|note| note.chars().count()).max().unwrap_or(0);
        // Columns and notes only when every row fits whole; otherwise each row is plain words, so
        // no version is ever cut off.
        let aligned = widths.0 + widths.1 + widest(|update| &update.to) + 9 + note + ROW_CHROME <= width;
        let mut items = Vec::new();
        for (title, updates) in [(t!("updates.repo"), repo), (t!("source.aur"), aur)] {
            let Some(updates) = updates.filter(|updates| !updates.is_empty()) else {
                continue;
            };
            if !items.is_empty() {
                items.push(ListItem::gap());
            }
            items.push(ListItem::header(title));
            items.extend(updates.iter().map(|update| row(update, widths, arrow, aligned)));
        }
        if let Some(failure) = aur_failure {
            if !items.is_empty() {
                items.push(ListItem::gap());
            }
            items.push(ListItem::header(t!("source.aur")));
            items.push(ListItem::new(t!("updates.aur-failed", reason = reason(failure))).faint(true));
        }
        items
    }
}

/// One update as a row: the name, the installed version, an arrow and the new version, in
/// columns as wide as the longest of each, and a quiet note where one applies. A list too narrow
/// for that drops the columns and the note, which would otherwise cut the versions off.
fn row(update: &Update, (name, from): (usize, usize), arrow: &str, aligned: bool) -> ListItem {
    let label = if aligned {
        format!("{:name$}  {:from$}  {arrow}  {}", update.name, update.from, update.to)
    } else {
        format!("{} {} {arrow} {}", update.name, update.from, update.to)
    };
    let item = ListItem::new(label);
    match note(update).filter(|_| aligned) {
        Some(note) => item.detail(note),
        None => item,
    }
}

/// The quiet note beside an update, where one applies: held back, or a restart needed.
fn note(update: &Update) -> Option<String> {
    if update.ignored {
        Some(t!("updates.held-back"))
    } else if needs_restart(&update.name) {
        Some(t!("updates.restart"))
    } else {
        None
    }
}

/// Why a check failed, for the user.
fn reason(failure: &Failure) -> String {
    match failure {
        Failure::NoPlace => t!("updates.no-place"),
        Failure::NoFakeroot => t!("updates.no-fakeroot"),
        Failure::Said(text) if text.is_empty() => t!("updates.no-reason"),
        Failure::Said(text) => text.clone(),
    }
}

/// `at` as `HH:MM`, `utc_offset` minutes from UTC.
fn clock(at: i64, utc_offset: i16) -> String {
    let moment = qframe::date::DateTime::from_unix(at, utc_offset);
    format!("{:02}:{:02}", moment.time.hour, moment.time.minute)
}

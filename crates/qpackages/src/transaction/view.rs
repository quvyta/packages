//! What the flow puts on screen: the confirmation, the lock notice and the output pane.

use std::time::{SystemTime, UNIX_EPOCH};

use qframe::prelude::*;
use qframe::widgets::{Badge, HoldToConfirm, LogView, Modal, ProgressBar, ScrollView};
use qpackages_core::lock::Owner;
use qpackages_core::pacman::{Plan, Step};

use super::{Action, Flow, Msg, State};
use crate::app::Msg as AppMsg;
use crate::detail::date_time_text;
use crate::packages::size_text;

/// Rows the step list takes before it scrolls inside the dialog.
const MAX_STEP_ROWS: u16 = 10;

/// Adds the confirmation or the lock notice while one is due. Either takes no room where it is
/// added; the framework draws it over everything.
pub fn modal(flow: &Flow, ui: &mut View<'_, AppMsg>) {
    match flow.state() {
        State::Confirming { action, plan, granted } => confirmation(action, plan, *granted, ui),
        State::Locked { since, owner } => locked(*since, owner.as_ref(), ui),
        _ => {}
    }
}

/// The plan as pacman printed it, the download it needs and whether a password will be asked,
/// with Cancel first so the safe answer has focus.
fn confirmation(action: &Action, plan: &Plan, granted: bool, ui: &mut View<'_, AppMsg>) {
    let n = plan.steps.len();
    let (title, apply, variant) = if action.is_removal() {
        (t!("transaction.remove-title", n = n), t!("transaction.remove"), "danger")
    } else {
        (t!("transaction.install-title", n = n), t!("transaction.install"), "primary")
    };
    let mut dialog = Modal::new()
        .title(title)
        .on_close(AppMsg::Transaction(Msg::Cancel))
        .action(Button::new(t!("transaction.cancel")).on_press(AppMsg::Transaction(Msg::Cancel)))
        .action(Button::new(apply).variant(variant).on_press(AppMsg::Transaction(Msg::Apply)));
    if action.is_removal() {
        dialog = dialog.variant("danger");
    }
    ui.add_with(dialog, |ui| {
        let lead = if action.is_removal() { t!("transaction.remove-lead") } else { t!("transaction.install-lead") };
        ui.add(Text::new(lead).role("secondary")).fill_width();
        let rows = u16::try_from(n).unwrap_or(u16::MAX);
        ui.add_with(ScrollView::new(), |ui| {
            ui.add(List::new(plan.steps.iter().map(step_item))).fill_width().height(Length::Cells(rows));
        })
        .fill_width()
        .height(Length::Cells(rows.clamp(1, MAX_STEP_ROWS)))
        .id("steps");
        if action.is_removal() {
            ui.add(Text::new(t!("transaction.remove-settings")).color("warning")).fill_width();
        } else {
            ui.add(Text::new(t!("transaction.download", size = size_text(plan.download()))).no_wrap());
        }
        let privilege = if granted { t!("transaction.privilege-granted") } else { t!("transaction.privilege-asked") };
        ui.add(Text::new(privilege).role("secondary")).fill_width();
    });
}

/// One step as pacman planned it: repository and name, the version, the download as the detail.
fn step_item(step: &Step) -> ListItem {
    let label = match &step.repo {
        Some(repo) => format!("{repo}/{}  {}", step.name, step.version),
        None => format!("{}  {}", step.name, step.version),
    };
    match step.size {
        Some(size) => ListItem::new(label).detail(size_text(size)),
        None => ListItem::new(label),
    }
}

/// Says the database is locked, by whom and since when as far as that can be told. There is no
/// way to remove the lock from here, by design.
fn locked(since: Option<SystemTime>, owner: Option<&Owner>, ui: &mut View<'_, AppMsg>) {
    let dialog = Modal::new()
        .title(t!("lock.title"))
        .on_close(AppMsg::Transaction(Msg::Cancel))
        .action(Button::new(t!("lock.close")).on_press(AppMsg::Transaction(Msg::Cancel)));
    ui.add_with(dialog, |ui| {
        ui.add(Badge::new(t!("lock.held")).variant("warning"));
        let owner_text = match owner {
            Some(Owner { pid, command: Some(command) }) => t!("lock.owner", command = command.as_str(), pid = *pid),
            Some(Owner { pid, command: None }) => t!("lock.owner-pid", pid = *pid),
            None => t!("lock.owner-unknown"),
        };
        ui.add(Text::new(owner_text)).fill_width();
        if let Some(unix) = since.and_then(unix_seconds) {
            ui.add(Text::new(t!("lock.since", time = date_time_text(unix))).no_wrap());
        }
        ui.add(Text::new(t!("lock.message")).role("secondary")).fill_width();
    });
}

/// Seconds since the epoch, or `None` for a time before it, which no lock file has.
fn unix_seconds(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH).ok().and_then(|since| i64::try_from(since.as_secs()).ok())
}

/// The output pane: a heading, the progress bar with the stop or close control, and pacman's
/// lines as they arrive.
pub fn output(flow: &Flow, ui: &mut View<'_, AppMsg>) {
    let (action, output, progress, running) = match flow.state() {
        State::Running { action, output, progress, .. } => (action, output, *progress, true),
        State::Finished { action, output } => (action, output, None, false),
        _ => return,
    };
    ui.column(|ui| {
        ui.row(|ui| {
            let heading = if running { super::running_label(action) } else { finished_label(action) };
            ui.add(Text::new(heading).bold().no_wrap());
            let bar = match (running, progress) {
                (true, Some(fraction)) => ProgressBar::new(fraction),
                (true, None) => ProgressBar::indeterminate(),
                (false, _) => ProgressBar::new(1.0).variant("danger").percent(false),
            };
            ui.add(bar).fill_width();
            if running {
                ui.add(HoldToConfirm::new(t!("transaction.stop")).on_confirm(AppMsg::Transaction(Msg::Stop)))
                    .id("stop");
            } else {
                ui.add(Button::new(t!("transaction.close")).on_press(AppMsg::Transaction(Msg::Close)))
                    .id("close-output");
            }
        })
        .gap(2)
        .fill_width();
        ui.add(LogView::new(output).empty_text(t!("transaction.waiting"))).fill().id("output");
    })
    .padding(Padding::symmetric(0, 1))
    .fill();
}

/// The heading of the pane after `action` did not go through.
fn finished_label(action: &Action) -> String {
    let names = action.names_text();
    if action.is_removal() {
        t!("transaction.finished-remove", names = names)
    } else {
        t!("transaction.finished-install", names = names)
    }
}

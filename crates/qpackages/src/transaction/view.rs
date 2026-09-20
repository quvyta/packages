//! What the flow puts on screen: the confirmation, the lock notice, the list of new `.pacnew`
//! files and the output pane.

use std::time::{SystemTime, UNIX_EPOCH};

use qframe::prelude::*;
use qframe::widgets::{Badge, HoldToConfirm, LogView, Modal, ProgressBar, ScrollView};
use qpackages_core::backup::{self, Tool};
use qpackages_core::build::{self as aur_build, Built};
use qpackages_core::flatpak::{FLATHUB, FLATHUB_REPO};
use qpackages_core::lock::Owner;
use qpackages_core::news::NewsItem;
use qpackages_core::pacman::{Plan, Step, Update};
use qpackages_core::reflector::Mirrors;
use qpackages_core::snap;

use super::{Action, Flow, Msg, State};
use crate::app::Msg as AppMsg;
use crate::detail::date_time_text;
use crate::installed::table::size_text;
use crate::updates::needs_restart;

/// Rows the step list takes before it scrolls inside the dialog.
const MAX_STEP_ROWS: u16 = 10;

/// Adds the confirmation, the lock notice or the list of new `.pacnew` files while one is due.
/// Each takes no room where it is added; the framework draws it over everything. `news` are the
/// recent Arch news that ask for the user's hand, which an update's confirmation repeats.
pub fn modal(flow: &Flow, news: &[NewsItem], ui: &mut View<'_, AppMsg>) {
    match flow.state() {
        State::Confirming { action, plan, granted, backup, pending, then, build } => {
            let facts = Facts {
                granted: *granted,
                backup: *backup,
                pending: *pending,
                news,
                then: then.len(),
                build: build.as_ref(),
                helper: flow.builder().map(|builder| builder.helper.program()),
            };
            confirmation(action, plan, &facts, ui);
        }
        State::Locked { since, owner } => locked(*since, owner.as_ref(), ui),
        _ => {
            if let Some(files) = flow.pacnew() {
                pacnew(files, ui);
            }
        }
    }
}

/// What the confirmation says beside the plan.
struct Facts<'a> {
    granted: bool,
    backup: backup::Plan,
    pending: usize,
    news: &'a [NewsItem],
    /// How many actions follow this one, each with its own confirmation.
    then: usize,
    /// What an AUR build builds and installs.
    build: Option<&'a aur_build::Plan>,
    /// The AUR helper that builds, by its program's name.
    helper: Option<&'static str>,
}

/// What happens around a system update under `plan`, for the Updates tab's last line and the
/// update's confirmation; `None` when no snapshot is taken, which needs no words.
#[must_use]
pub fn backup_note(plan: backup::Plan) -> Option<String> {
    match plan {
        backup::Plan::Off => None,
        backup::Plan::SnapPac => Some(t!("backup.snap-pac")),
        backup::Plan::Unavailable(tool) => Some(t!("backup.chosen-missing", tool = tool.key())),
        backup::Plan::Take(Tool::Snapper) => Some(t!("backup.snapper")),
        backup::Plan::Take(Tool::Timeshift) => Some(t!("backup.timeshift")),
    }
}

/// The dialog's usual width, which fits two buttons.
const DIALOG_WIDTH: u16 = 56;

/// A width that keeps every button of the row whole: the dialog's buttons sit on one line and
/// a row wider than the dialog would lose the first ones off its left edge. Each button carries
/// two cells of padding a side, two cells lie between buttons, and the dialog keeps ten cells
/// for its padding, its close mark and some air.
fn buttons_width(labels: &[&str]) -> u16 {
    let buttons: u16 = labels.iter().map(|label| qframe::text::width(label).saturating_add(4)).sum();
    let gaps = 2 * u16::try_from(labels.len().saturating_sub(1)).unwrap_or(0);
    DIALOG_WIDTH.max(buttons.saturating_add(gaps).saturating_add(10))
}

/// The plan as pacman printed it, or the updates the check found, or the Flatpak applications by
/// id, the download it needs and whether a password will be asked, with Cancel first so the safe
/// answer has focus.
fn confirmation(action: &Action, plan: &Plan, facts: &Facts<'_>, ui: &mut View<'_, AppMsg>) {
    let n = plan.steps.len();
    let ids = action.names().len();
    let cancel = Button::new(t!("transaction.cancel")).on_press(AppMsg::Transaction(Msg::Cancel));
    let apply =
        |label: String, variant: &str| Button::new(label).variant(variant).on_press(AppMsg::Transaction(Msg::Apply));
    let (title, lead) = match action {
        Action::Install(_) | Action::UpgradeInstall(_) => {
            (t!("transaction.install-title", n = n), t!("transaction.install-lead"))
        }
        Action::Remove(_) => (t!("transaction.remove-title", n = n), t!("transaction.remove-lead")),
        Action::RemoveOrphans(_) => (t!("orphans.remove-title", n = n), t!("orphans.remove-lead")),
        Action::Upgrade(updates) => {
            (t!("transaction.upgrade-title", n = updates.len()), t!("transaction.upgrade-lead"))
        }
        Action::Mirrors(_) => (t!("mirrors.apply-title"), t!("mirrors.apply-lead")),
        Action::Timer(true) => (t!("mirrors.timer-on-title"), t!("mirrors.timer-on-lead")),
        Action::Timer(false) => (t!("mirrors.timer-off-title"), t!("mirrors.timer-off-lead")),
        Action::FlatpakInstall(_) => (t!("flatpak.install-title", n = ids), t!("flatpak.install-lead")),
        Action::FlatpakRemove(_) => (t!("flatpak.remove-title", n = ids), t!("flatpak.remove-lead")),
        Action::FlatpakRemoveSystem(_) => (t!("flatpak.remove-title", n = ids), t!("flatpak.remove-system-lead")),
        Action::AddFlathub => (t!("flatpak.add-title"), t!("flatpak.add-lead")),
        Action::AurInstall(_) => {
            let builds = facts.build.map_or(0, |build| build.builds.len());
            (t!("aur.build-title", n = builds), t!("aur.build-lead", helper = facts.helper.unwrap_or_default()))
        }
        Action::SnapInstall(_) => (t!("snap.install-title", n = ids), t!("snap.install-lead")),
        Action::SnapInstallClassic(_) => (t!("snap.install-title", n = ids), t!("snap.install-classic-lead")),
        Action::SnapRemove(_) => (t!("snap.remove-title", n = ids), t!("snap.remove-lead")),
        Action::SnapRefresh(_) => (t!("snap.refresh-title", n = ids), t!("snap.refresh-lead")),
        Action::SnapdSocket(true) => (t!("snap.socket-on-title"), t!("snap.socket-on-lead")),
        Action::SnapdSocket(false) => (t!("snap.socket-off-title"), t!("snap.socket-off-lead")),
        Action::SnapLink => (t!("snap.link-title"), t!("snap.link-lead")),
    };
    let mut dialog = Modal::new().title(title).on_close(AppMsg::Transaction(Msg::Cancel)).action(cancel);
    dialog = match action {
        Action::Remove(_)
        | Action::RemoveOrphans(_)
        | Action::FlatpakRemove(_)
        | Action::FlatpakRemoveSystem(_)
        | Action::SnapRemove(_) => dialog.variant("danger").action(apply(t!("transaction.remove"), "danger")),
        Action::Install(_) if facts.pending > 0 => {
            let (only, both) = (t!("transaction.install-only"), t!("transaction.upgrade-and-install"));
            dialog
                .width(buttons_width(&[&t!("transaction.cancel"), &only, &both]))
                .action(Button::new(only).on_press(AppMsg::Transaction(Msg::Apply)))
                .action(Button::new(both).variant("primary").on_press(AppMsg::Transaction(Msg::ApplyWithUpgrade)))
        }
        Action::Install(_) | Action::UpgradeInstall(_) | Action::FlatpakInstall(_) => {
            dialog.action(apply(t!("transaction.install"), "primary"))
        }
        Action::AddFlathub => dialog.action(apply(t!("flatpak.add"), "primary")),
        Action::AurInstall(_) => dialog.action(apply(t!("aur.build"), "primary")),
        Action::Upgrade(_) => dialog.action(apply(t!("transaction.upgrade"), "primary")),
        Action::Mirrors(_) => dialog.action(apply(t!("mirrors.apply"), "primary")),
        Action::Timer(true) => dialog.action(apply(t!("mirrors.timer-on"), "primary")),
        Action::Timer(false) => dialog.action(apply(t!("mirrors.timer-off"), "primary")),
        Action::SnapInstall(_) | Action::SnapInstallClassic(_) => {
            dialog.action(apply(t!("transaction.install"), "primary"))
        }
        Action::SnapRefresh(_) => dialog.action(apply(t!("snap.refresh"), "primary")),
        Action::SnapdSocket(true) => dialog.action(apply(t!("snap.socket-on"), "primary")),
        Action::SnapdSocket(false) => dialog.action(apply(t!("snap.socket-off"), "primary")),
        Action::SnapLink => dialog.action(apply(t!("snap.link"), "primary")),
    };
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(lead).role("secondary")).fill_width();
        let arrow = ui.env().icons().glyph("arrow-right").into_owned();
        let items: Vec<ListItem> = match action {
            Action::Upgrade(updates) => updates.iter().map(|update| update_item(update, &arrow)).collect(),
            Action::Mirrors(mirrors) => mirror_lines(mirrors).into_iter().map(ListItem::new).collect(),
            Action::Timer(_) => Vec::new(),
            Action::FlatpakInstall(names) | Action::FlatpakRemove(names) | Action::FlatpakRemoveSystem(names) => {
                names.iter().map(ListItem::new).collect()
            }
            Action::AddFlathub => vec![ListItem::new(FLATHUB).detail(FLATHUB_REPO)],
            Action::SnapInstall(names)
            | Action::SnapInstallClassic(names)
            | Action::SnapRemove(names)
            | Action::SnapRefresh(names) => names.iter().map(ListItem::new).collect(),
            Action::SnapdSocket(_) => vec![ListItem::new(snap::SOCKET_UNIT)],
            Action::SnapLink => vec![ListItem::new(snap::SNAP_LINK).detail(snap::SNAP_DIR)],
            Action::AurInstall(_) => {
                let builds = facts.build.map(|build| build.builds.as_slice()).unwrap_or_default();
                builds.iter().map(built_item).chain(plan.steps.iter().map(step_item)).collect()
            }
            _ => plan.steps.iter().map(step_item).collect(),
        };
        if !items.is_empty() {
            let rows = u16::try_from(items.len()).unwrap_or(u16::MAX);
            ui.add_with(ScrollView::new(), |ui| {
                ui.add(List::new(items)).fill_width().height(Length::Cells(rows));
            })
            .fill_width()
            .height(Length::Cells(rows.clamp(1, MAX_STEP_ROWS)))
            .id("steps");
        }
        match action {
            Action::Remove(_) | Action::RemoveOrphans(_) => {
                ui.add(Text::new(t!("transaction.remove-settings")).color("warning")).fill_width();
            }
            Action::Install(_) | Action::UpgradeInstall(_) => {
                ui.add(Text::new(t!("transaction.download", size = size_text(plan.download()))).no_wrap());
                if facts.pending > 0 {
                    ui.add(Text::new(t!("transaction.pending", n = facts.pending)).color("warning")).fill_width();
                }
            }
            Action::Upgrade(_) => {
                for item in facts.news.iter().filter(|item| item.manual_intervention) {
                    ui.add(Text::new(t!("news.manual", title = item.title.as_str())).color("warning")).fill_width();
                }
                if let Some(note) = backup_note(facts.backup) {
                    ui.add(Text::new(note)).fill_width();
                }
            }
            Action::Mirrors(_) => {
                ui.add(Text::new(t!("mirrors.apply-keeps")).role("secondary")).fill_width();
            }
            Action::Timer(_) | Action::AddFlathub | Action::FlatpakInstall(_) | Action::SnapdSocket(_) => {}
            Action::SnapInstall(_) | Action::SnapRefresh(_) => {
                // snapd decides which bases come along and says so only while it works.
                ui.add(Text::new(t!("snap.size-unknown")).role("secondary")).fill_width();
            }
            Action::SnapInstallClassic(_) => {
                ui.add(Text::new(t!("snap.classic-warning")).color("warning")).fill_width();
                ui.add(Text::new(t!("snap.size-unknown")).role("secondary")).fill_width();
            }
            Action::SnapRemove(_) => {
                ui.add(Text::new(t!("snap.remove-keeps-data")).role("secondary")).fill_width();
                ui.add(Text::new(t!("snap.remove-keeps-bases")).role("secondary")).fill_width();
            }
            Action::SnapLink => {
                ui.add(Text::new(t!("snap.link-note")).role("secondary")).fill_width();
            }
            Action::FlatpakRemove(_) => {
                ui.add(Text::new(t!("flatpak.remove-unused")).color("warning")).fill_width();
                ui.add(Text::new(t!("flatpak.remove-keeps-data")).role("secondary")).fill_width();
            }
            Action::FlatpakRemoveSystem(_) => {
                ui.add(Text::new(t!("flatpak.remove-keeps-data")).role("secondary")).fill_width();
            }
            Action::AurInstall(_) => {
                if !plan.steps.is_empty() {
                    ui.add(Text::new(t!("transaction.download", size = size_text(plan.download()))).no_wrap());
                    ui.add(Text::new(t!("aur.make-removed")).role("secondary")).fill_width();
                }
                ui.add(Text::new(t!("aur.warning")).color("warning")).fill_width();
            }
        }
        if facts.then > 0 {
            ui.add(Text::new(t!("transaction.then", n = facts.then))).fill_width();
        }
        let privilege = if action.as_user() {
            t!("transaction.privilege-none")
        } else if facts.granted {
            t!("transaction.privilege-granted")
        } else {
            t!("transaction.privilege-asked")
        };
        ui.add(Text::new(privilege).role("secondary")).fill_width();
    });
}

/// One update as the confirmation lists it: the name and the two versions, and the restart
/// note where it applies.
fn update_item(update: &Update, arrow: &str) -> ListItem {
    let item = ListItem::new(format!("{}  {} {arrow} {}", update.name, update.from, update.to));
    if needs_restart(&update.name) { item.detail(t!("updates.restart")) } else { item }
}

/// The mirror choices, one line each, as the confirmation lists them.
fn mirror_lines(mirrors: &Mirrors) -> Vec<String> {
    let countries = mirrors.countries();
    let countries = if countries.is_empty() { t!("mirrors.every-country") } else { countries.join(", ") };
    vec![
        t!("mirrors.line-countries", countries = countries),
        t!("mirrors.line-count", n = u32::from(mirrors.count()), hours = u32::from(mirrors.age())),
        t!("mirrors.line-sort", sort = t!(&format!("mirrors.sort-{}", mirrors.sort().key()))),
    ]
}

/// The `.pacnew` files the last transaction left, with what they are, until closed.
fn pacnew(files: &[String], ui: &mut View<'_, AppMsg>) {
    let close = Button::new(t!("transaction.close")).on_press(AppMsg::Transaction(Msg::ClosePacnew));
    let dialog = Modal::new()
        .title(t!("pacnew.title", n = files.len()))
        .on_close(AppMsg::Transaction(Msg::ClosePacnew))
        .action(close);
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(t!("pacnew.lead")).role("secondary")).fill_width();
        let rows = u16::try_from(files.len()).unwrap_or(u16::MAX);
        ui.add_with(ScrollView::new(), |ui| {
            ui.add(List::new(files.iter().map(ListItem::new))).fill_width().height(Length::Cells(rows));
        })
        .fill_width()
        .height(Length::Cells(rows.clamp(1, MAX_STEP_ROWS)))
        .id("pacnew");
        ui.add(Text::new(t!("pacnew.hint")).role("secondary")).fill_width();
    });
}

/// One package the build makes from the AUR: its name and version, the AUR as its source.
fn built_item(built: &Built) -> ListItem {
    ListItem::new(format!("aur/{}  {}", built.name, built.version)).detail(t!("aur.to-build"))
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

/// The output pane: a heading, the progress bar with the stop or close control, and what the
/// helper's programs said as it arrives.
pub fn output(flow: &Flow, ui: &mut View<'_, AppMsg>) {
    let (State::Running { job, .. } | State::Finished(job) | State::BackupFailed(job)) = flow.state() else {
        return;
    };
    ui.column(|ui| {
        ui.row(|ui| {
            // A job waiting for the answer about its snapshot keeps its heading: it is paused,
            // not over, and the bar says so in the warning tone.
            let (heading, bar) = match flow.state() {
                State::Running { job, .. } => {
                    let bar = job.progress.map_or_else(ProgressBar::indeterminate, ProgressBar::new);
                    (job.heading(), bar)
                }
                State::BackupFailed(job) => (job.heading(), ProgressBar::new(1.0).variant("warning").percent(false)),
                _ => (finished_label(&job.action), ProgressBar::new(1.0).variant("danger").percent(false)),
            };
            // The heading and the bar share what the control at the end leaves: measured on
            // their own, a heading as long as a translation can make it would take the whole row
            // and the control would be cut off at the edge. In their own filling row the control
            // keeps its width and the heading is the one that shortens.
            ui.row(|ui| {
                ui.add(Text::new(heading).bold().no_wrap());
                ui.add(bar).fill_width();
            })
            .gap(2)
            .fill_width();
            match flow.state() {
                State::Running { .. } => {
                    ui.add(HoldToConfirm::new(t!("transaction.stop")).on_confirm(AppMsg::Transaction(Msg::Stop)))
                        .id("stop");
                }
                State::Finished(_) => {
                    ui.add(Button::new(t!("transaction.close")).on_press(AppMsg::Transaction(Msg::Close)))
                        .id("close-output");
                }
                _ => {}
            }
        })
        .gap(2)
        .fill_width();
        let waiting = if job.action.runs_flatpak() {
            t!("transaction.waiting-flatpak")
        } else if matches!(job.action, Action::AurInstall(_)) {
            t!("aur.waiting")
        } else {
            t!("transaction.waiting")
        };
        ui.add(LogView::new(&job.output).empty_text(waiting)).fill().id("output");
    })
    .padding(Padding::symmetric(0, 1))
    .fill();
}

/// The heading of the pane after `action` did not go through.
fn finished_label(action: &Action) -> String {
    let names = action.names_text();
    match action {
        Action::Install(_) => t!("transaction.finished-install", names = names),
        Action::Remove(_) => t!("transaction.finished-remove", names = names),
        Action::Upgrade(_) | Action::UpgradeInstall(_) => t!("transaction.finished-upgrade"),
        Action::RemoveOrphans(_) => t!("transaction.finished-orphans"),
        Action::Mirrors(_) => t!("transaction.finished-mirrors"),
        Action::Timer(_) => t!("transaction.finished-timer"),
        Action::FlatpakInstall(_) => t!("transaction.finished-install", names = names),
        Action::FlatpakRemove(_) | Action::FlatpakRemoveSystem(_) => t!("transaction.finished-remove", names = names),
        Action::AddFlathub => t!("flatpak.finished-add"),
        Action::AurInstall(_) => t!("aur.finished", names = names),
        Action::SnapInstall(_) | Action::SnapInstallClassic(_) => t!("snap.finished-install", names = names),
        Action::SnapRemove(_) => t!("snap.finished-remove", names = names),
        Action::SnapRefresh(_) => t!("snap.finished-refresh", names = names),
        Action::SnapdSocket(_) => t!("snap.finished-socket"),
        Action::SnapLink => t!("snap.finished-link"),
    }
}

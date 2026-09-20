//! How the Installed tab looks: the search and the view choice on top, the table with the
//! details beside it (or instead of it on a narrow screen), and a summary line with the removal
//! at the bottom.

use std::collections::BTreeSet;

use qframe::prelude::*;
use qframe::text;
use qframe::widgets::{Segmented, Splitter, Table, TextInput};

use super::table::{self, RowsKey};
use super::{Installed, Library, Msg, Show};
use crate::detail;

/// Screens narrower than this open the details in place of the table instead of beside it.
pub const DETAIL_BESIDE: u16 = 100;

/// Screens narrower than this put the view choice on a line of its own under the search.
const CHOICE_BESIDE: u16 = 60;

/// Screens narrower than this leave the source column out.
const SOURCE_SHOWN: u16 = 40;

/// Columns plain lines of text stand in from the edge, level with the search field's text and the
/// header's name.
pub const TEXT_INDENT: u16 = 2;

/// The narrowest the table and the detail panel get before one gives way to the other.
const MIN_TABLE: u16 = 24;
const MIN_DETAIL: u16 = 24;

/// What the tab needs from the rest of the application to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Cx<'a> {
    /// The installed packages.
    pub library: Library<'a>,
    /// How many records of the local database could not be read.
    pub problems: usize,
    /// Whether the first read is still on its way.
    pub loading: bool,
    /// Whether a removal is being planned, so the button that asked shows it.
    pub planning: bool,
}

impl Installed {
    /// Draws the tab.
    pub fn view(&self, ui: &mut View<'_, Msg>, cx: Cx<'_>) {
        let width = ui.size().width;
        ui.column(|ui| {
            self.search_row(width, ui);
            if cx.problems > 0 {
                ui.add(Text::new(t!("packages.unreadable", n = cx.problems)).role("faint").no_wrap())
                    .fill_width()
                    .padding(Padding::symmetric(0, TEXT_INDENT));
            }
            let package = self.selected_row(cx.library).map(|row| &cx.library.packages[self.shown[row]]);
            if width >= DETAIL_BESIDE {
                let widest = width.saturating_sub(MIN_DETAIL).max(MIN_TABLE);
                // Until dragged, the table takes three fifths: its five columns read in full and
                // the details keep room for a description.
                let split = self.split.unwrap_or(width / 5 * 3);
                Splitter::columns(split.clamp(MIN_TABLE, widest))
                    .limits(MIN_TABLE, widest)
                    .on_resize(Msg::Split)
                    .first(|ui| self.table(width, cx, ui))
                    .second(|ui| detail::view(package, ui))
                    .show(ui);
            } else if self.detail_open
                && let Some(package) = package
            {
                ui.column(|ui| {
                    let back = Button::new(t!("installed.back")).icon("arrow-left").on_press(Msg::CloseDetail);
                    ui.add(back).id("detail-back");
                    detail::view(Some(package), ui);
                })
                .gap(1)
                .fill();
            } else {
                self.table(width, cx, ui);
            }
            self.summary(width, cx, ui);
        })
        .fill();
    }

    /// The search field and, beside it or under it, the choice between applications and every
    /// package.
    fn search_row(&self, width: u16, ui: &mut View<'_, Msg>) {
        let search = TextInput::new(self.search.clone()).placeholder(t!("installed.search")).on_change(Msg::Search);
        let chosen = Show::ALL.iter().position(|show| *show == self.show).unwrap_or(0);
        let choice = Segmented::new([t!("installed.apps"), t!("installed.all")]).selected(chosen).on_select(Msg::Show);
        if width >= CHOICE_BESIDE {
            ui.row(|ui| {
                ui.add(search).fill_width().id("search");
                ui.add(choice).id("show");
            })
            .gap(2)
            .fill_width();
        } else {
            ui.add(search).fill_width().id("search");
            ui.add(choice).id("show");
        }
    }

    fn table(&self, width: u16, cx: Cx<'_>, ui: &mut View<'_, Msg>) {
        let narrow = width < SOURCE_SHOWN;
        let cols = table::visible(narrow);
        let empty = if cx.loading {
            t!("packages.loading")
        } else if !self.search.is_empty() {
            t!("packages.none-match")
        } else if self.show == Show::Apps {
            t!("installed.no-apps")
        } else {
            t!("packages.none-installed")
        };
        let key = RowsKey::new(ui.env().icons().mode(), narrow);
        let rows = self.rows.get(key, |key| table::rows(cx.library, &self.shown, key));
        let (col, direction) = self.sort;
        let sort_index = cols.iter().position(|shown| *shown == col).unwrap_or(0);
        let table = Table::new(table::columns(cols), rows)
            .selected(self.selected_row(cx.library))
            .checked(self.marks.clone())
            .sort(sort_index, direction)
            .empty_text(empty)
            .on_select(Msg::Select)
            .on_activate(Msg::Open)
            .on_toggle(Msg::Toggle)
            .on_sort(move |index, direction| Msg::Sort(cols.get(index).copied().unwrap_or(col), direction));
        ui.add(table).fill().id("packages");
    }

    /// How many applications and packages there are, how many are checked, and the removal.
    ///
    /// The counts give way with an ellipsis when the row is short of cells, but a button cannot:
    /// a shortened "Remove checked" is a button nobody dares press. So when what stands to the
    /// right of the counts no longer fits beside them, it leaves the row and stands under them,
    /// and when the buttons cannot share a row either, each takes one of its own.
    fn summary(&self, width: u16, cx: Cx<'_>, ui: &mut View<'_, Msg>) {
        if cx.loading {
            return;
        }
        let total = cx.library.packages.len();
        let mut parts = Vec::new();
        match self.show {
            Show::Apps => {
                parts.push(t!("installed.count-apps", n = self.shown.len()));
                parts.push(t!("installed.count-packages", n = total));
            }
            Show::All if self.search.is_empty() => parts.push(t!("installed.count-packages", n = total)),
            Show::All => {
                parts.push(t!("installed.count-matching", n = self.shown.len()));
                parts.push(t!("installed.count-packages", n = total));
            }
        }
        if !self.checked.is_empty() {
            parts.push(t!("transaction.checked", n = self.checked.len()));
        }
        // Orphans are a matter of every package, where they are marked; among the applications
        // they would be a count of rows that are not there.
        let orphans = cx.library.orphans.as_ref().map_or(0, BTreeSet::len);
        let clean = self.show == Show::All && orphans > 0;
        let orphan_text = clean.then(|| t!("installed.orphans", n = orphans));
        let clean_label = clean.then(|| t!("installed.clean-up"));
        let remove_label = (!self.checked.is_empty()).then(|| t!("installed.remove-checked"));
        let button = |label: &Option<String>| label.as_deref().map_or(0, |label| button_width(ui, label) + GAP);
        let buttons = button(&clean_label) + button(&remove_label);
        let beside = orphan_text.as_deref().map_or(0, |text| text::width(text) + GAP) + buttons;
        let room = width.saturating_sub(TEXT_INDENT.saturating_mul(2));
        let add_buttons = |ui: &mut View<'_, Msg>| {
            if let Some(label) = clean_label.clone() {
                let button = Button::new(label).loading(cx.planning).on_press(Msg::CleanOrphans);
                ui.add(button).id("clean-orphans");
            }
            if let Some(label) = remove_label.clone() {
                let remove = Button::new(label).variant("danger").loading(cx.planning).on_press(Msg::RemoveChecked);
                ui.add(remove).id("remove");
            }
        };
        if beside <= room {
            ui.row(|ui| {
                ui.add(Text::new(parts.join(" · ")).role("faint").no_wrap()).fill_width();
                if let Some(text) = orphan_text.clone() {
                    ui.add(Text::new(text).role("faint").no_wrap());
                }
                add_buttons(ui);
            })
            .gap(GAP)
            .padding(Padding::symmetric(0, TEXT_INDENT))
            .fill_width();
            return;
        }
        // The orphan count joins the other counts: it is a count, and the buttons have left.
        if let Some(text) = orphan_text {
            parts.push(text);
        }
        ui.column(|ui| {
            ui.add(Text::new(parts.join(" · ")).role("faint").no_wrap()).fill_width();
            if buttons <= room {
                ui.row(|ui| add_buttons(ui)).gap(GAP).fill_width().justify(Align::End);
            } else {
                ui.column(|ui| add_buttons(ui)).gap(0).fill_width().align(Align::End);
            }
        })
        .gap(0)
        .padding(Padding::symmetric(0, TEXT_INDENT))
        .fill_width();
    }
}

/// Cells between the things standing on the summary row.
const GAP: u16 = 2;

/// Cells a button labelled `label` takes: its label with the theme's own padding on each side.
///
/// The summary row has to know this before it places anything, because whether the buttons fit
/// beside the counts is what decides the shape of the row.
fn button_width(ui: &View<'_, Msg>, label: &str) -> u16 {
    let style = ui.env().theme().style("button", None, &[]);
    let (_, horizontal) = style.pair("padding").unwrap_or((0, 2));
    text::width(label).saturating_add(horizontal.saturating_mul(2))
}

//! How the Discover page is laid out: the search field, the kinds column with the sources under
//! it, and on the right either the home rows or the search results.

use std::rc::Rc;

use qframe::prelude::*;
use qframe::widgets::{EmptyState, PageTransition, Select, Spinner, TextInput};
use qpackages_core::catalog::merge::Offer;
use qpackages_core::sources::{Availability, Source};

use super::card;
use super::model::{Card, Kind, Sort, kind_counts};
use super::{CATALOG_PACKAGE, Grid, MAX_RESULTS, Msg, Page, Request, Search, Section, Store, page};

/// Width of the kinds column (design 3.2).
pub const KINDS_WIDTH: u16 = 18;

/// Cells a list row takes besides its label and detail: the pillar, the icon, the gaps and the
/// cell a lit row slides by.
const ROW_OVERHEAD: u16 = 8;

/// Cells between the kinds column and the cards.
const KINDS_GAP: u16 = 2;

/// Below this width the kinds column gives way to a one-line choice under the search field.
pub const NARROW: u16 = 80;

/// Below this height the home page shows one row of cards.
pub const SHORT: u16 = 24;

/// Draws the page the store is on, cross-fading when it changes.
pub fn show(store: &Store, ui: &mut View<'_, Msg>) {
    let page = store.router.current().clone();
    let key = page.key();
    ui.add_with(PageTransition::new(key.clone()).direction(store.router.direction()), |ui| {
        ui.page(key, |ui| match &page {
            Page::Home => discover(store, ui),
            Page::Section(section) => section_page(store, *section, ui),
            Page::App(_) => page::show(store, ui),
        })
        .fill();
    })
    .fill();
}

/// The home page, or the results while there is a search.
fn discover(store: &Store, ui: &mut View<'_, Msg>) {
    let narrow = ui.size().width < NARROW;
    ui.column(|ui| {
        search_field(store, ui);
        if narrow {
            kind_select(store, ui);
        }
        ui.row(|ui| {
            if !narrow {
                ui.column(|ui| kinds_column(store, ui)).width(Length::Cells(KINDS_WIDTH)).fill_height();
            }
            ui.column(|ui| match &store.search {
                Some(search) => results(store, search, ui),
                None => home(store, narrow, ui),
            })
            .gap(1)
            .fill();
        })
        .gap(KINDS_GAP)
        .fill();
    })
    .gap(1)
    .fill();
}

fn search_field(store: &Store, ui: &mut View<'_, Msg>) {
    ui.add(TextInput::new(store.query.clone()).placeholder(t!("store.search.placeholder")).on_change(Msg::Query))
        .fill_width()
        .id("store-search");
}

/// The kind as one line under the search field, for screens too narrow for the column.
fn kind_select(store: &Store, ui: &mut View<'_, Msg>) {
    let kinds = store.kinds();
    let selected = kinds.iter().position(|&kind| kind == store.kind);
    ui.row(|ui| {
        ui.add(Text::new(t!("store.kind.label")).color("muted").no_wrap());
        ui.add(Select::new(kinds.iter().map(|kind| t!(&kind.key()))).selected(selected).on_select(Msg::Kind))
            .fill_width()
            .id("store-kind");
    })
    .gap(1)
    .fill_width();
}

/// One line offering the repositories' catalog when it is missing: without it the kinds and
/// the repositories' summaries stay empty. It stands under the rows, so appearing once the
/// catalog was looked for moves nothing already on screen.
fn catalog_line(store: &Store, ui: &mut View<'_, Msg>) {
    let missing =
        store.loaded.as_ref().is_some_and(|loaded| !loaded.has_repo_catalog) && store.is_enabled(Source::Pacman);
    if !missing {
        return;
    }
    let install = Request::Install(vec![Offer { source: Source::Pacman, package: CATALOG_PACKAGE.to_owned() }]);
    ui.row(|ui| {
        ui.add(Text::new(t!("store.catalog.missing")).color("muted")).fill_width();
        ui.add(Button::new(t!("store.catalog.install")).on_press(Msg::Request(install))).id("store-catalog");
    })
    .gap(2)
    .fill_width();
}

fn kinds_column(store: &Store, ui: &mut View<'_, Msg>) {
    let kinds = store.kinds();
    let counts = store.search.as_ref().map(|search| kind_counts(&search.ranked));
    let items = kinds.iter().map(|kind| {
        let item = ListItem::new(t!(&kind.key()));
        match &counts {
            Some(counts) => item.detail(counts.get(kind).copied().unwrap_or(0).to_string()),
            None => item,
        }
    });
    let selected = kinds.iter().position(|&kind| kind == store.kind);
    let rows = u16::try_from(kinds.len()).unwrap_or(u16::MAX);
    ui.add(List::new(items).selected(selected).on_select(Msg::Kind))
        .height(Length::Cells(rows))
        .fill_width()
        .id("store-kinds");
    ui.add(Text::new("")).height(Length::Cells(1));
    ui.add(Text::new(t!("store.sources.title")).color("muted").no_wrap()).fill_width();
    let sources = store.enabled.iter().map(|&source| {
        let label = t!(&format!("store.source.{}", crate::sources::name(source)));
        match store.sources.as_ref().map(|sources| sources.get(source)) {
            Some(Availability::Ready { .. }) => ListItem::new(label).icon("check", Some("success")),
            // Faint, with the word saying why: colour alone never does.
            Some(Availability::Missing) => {
                let item = ListItem::new(label.clone()).icon("dot-outline", None).faint(true);
                // "Install" beside the name where the column has room for both; the hollow mark
                // and the faint row still say it where it has not.
                let install = t!("store.sources.install");
                let fits = qframe::text::width(&label) + qframe::text::width(&install) + ROW_OVERHEAD <= KINDS_WIDTH;
                if fits { item.detail(install) } else { item }
            }
            // Nothing is said about a source before the application has looked for it.
            None => ListItem::new(label),
        }
    });
    ui.add(List::new(sources).selected(store.source_row).on_select(Msg::SourceSelect).on_activate(Msg::SourceRow))
        .fill()
        .id("store-sources");
}

/// The rows of cards before anything is typed.
fn home(store: &Store, narrow: bool, ui: &mut View<'_, Msg>) {
    let size = ui.size();
    let width = if narrow { size.width } else { size.width.saturating_sub(KINDS_WIDTH + KINDS_GAP) };
    let per_row = card::columns(width);
    row(store, Section::Popular, per_row, ui);
    if size.height >= SHORT && !store.aur_row.is_empty() {
        row(store, Section::Aur, per_row, ui);
    }
    catalog_line(store, ui);
}

/// A section's title with the way to its whole list, then one row of its cards.
fn row(store: &Store, section: Section, per_row: usize, ui: &mut View<'_, Msg>) {
    let grid = Grid::Row(section);
    let all = store.grid_cards(grid);
    let shown: Rc<[Card]> = all.iter().take(per_row).cloned().collect();
    ui.row(|ui| {
        ui.add(Text::new(section_title(store, section)).bold().no_wrap()).fill_width();
        if all.len() > shown.len() {
            ui.add(Button::new(t!("store.home.see-all")).on_press(Msg::SeeAll(section)))
                .id(format!("store-see-all-{}", section_id(section)));
        }
    })
    .gap(2)
    .fill_width();
    if shown.is_empty() {
        ui.add(Text::new(t!("store.home.empty-kind")).color("muted")).fill_width();
        return;
    }
    let cards =
        card::grid(ui, &store.installed, grid, &shown, selected(store, grid, &shown), checked(store, &shown), None);
    ui.add(cards).fill_width().id(format!("store-row-{}", section_id(section)));
}

fn section_id(section: Section) -> &'static str {
    match section {
        Section::Popular => "popular",
        Section::Aur => "aur",
    }
}

/// "Popular apps", or "Internet: popular" on a kind's page.
fn section_title(store: &Store, section: Section) -> String {
    match (section, store.kind) {
        (Section::Popular, Kind::All) => t!("store.home.popular"),
        (Section::Aur, Kind::All) => t!("store.home.aur"),
        (Section::Popular, kind) => t!("store.home.kind-popular", kind = t!(&kind.key())),
        (Section::Aur, kind) => t!("store.home.kind-aur", kind = t!(&kind.key())),
    }
}

/// A section's whole list, from "See all".
fn section_page(store: &Store, section: Section, ui: &mut View<'_, Msg>) {
    let grid = Grid::All(section);
    let cards = store.grid_cards(grid);
    ui.column(|ui| {
        back_button(ui);
        ui.add(Text::new(section_title(store, section)).bold().no_wrap()).fill_width();
        let built =
            card::grid(ui, &store.installed, grid, &cards, selected(store, grid, &cards), checked(store, &cards), None);
        ui.add(built).fill().id("store-all");
    })
    .gap(1)
    .fill();
}

/// `‹ Back`, which `esc` also does.
pub fn back_button(ui: &mut View<'_, Msg>) {
    ui.row(|ui| {
        ui.add(Button::new(t!("store.back")).icon("chevron-left").on_press(Msg::Back)).id("store-back");
    });
}

/// The search results with their sort, the sources still searching and the ones that failed.
fn results(store: &Store, search: &Search, ui: &mut View<'_, Msg>) {
    let total = search.ranked.len();
    ui.row(|ui| {
        ui.add(Text::new(t!("store.search.count", n = total, query = search.query.clone())).bold().no_wrap())
            .fill_width();
        ui.add(Text::new(t!("store.sort.label")).color("muted").no_wrap());
        let sorts = Sort::ALL.map(|sort| t!(sort.key()));
        let current = Sort::ALL.iter().position(|&sort| sort == store.sort);
        ui.add(Select::new(sorts).selected(current).on_select(Msg::Sort)).width(Length::Cells(16)).id("store-sort");
    })
    .gap(1)
    .fill_width();
    let cards = &store.results;
    // "Nothing found" is said only once every source has answered.
    let empty = search.settled().then(|| {
        EmptyState::new(t!("store.search.none-title", query = search.query.clone()))
            .icon("search")
            .message(t!("store.search.none-message"))
    });
    let built = card::grid(
        ui,
        &store.installed,
        Grid::Results,
        cards,
        selected(store, Grid::Results, cards),
        checked(store, cards),
        empty,
    );
    ui.add(built).fill().id("store-results");
    let shown_of_kind = search.ranked.iter().filter(|card| store.kind.holds(card.app.category)).count();
    if shown_of_kind > MAX_RESULTS {
        ui.add(Text::new(t!("store.search.more", shown = MAX_RESULTS, n = shown_of_kind)).color("muted")).fill_width();
    }
    if search.spinner && !search.pending.is_empty() {
        let names: Vec<String> = search.pending.iter().map(|&source| source_name(source)).collect();
        ui.add(Spinner::new().label(t!("store.search.searching", sources = names.join(", ")))).fill_width();
    }
    for (source, failure) in &search.failed {
        failure_line(*source, failure, ui);
    }
}

/// A source's name as the store writes it.
fn source_name(source: Source) -> String {
    t!(&format!("store.source.{}", crate::sources::name(source)))
}

/// Why a source gave nothing, in the warning tone, with a way to ask again where asking again
/// can help.
fn failure_line(source: Source, failure: &super::Failure, ui: &mut View<'_, Msg>) {
    let name = source_name(source);
    let (text, retry) = match failure {
        super::Failure::TooMany => (t!("store.search.too-many", source = name), false),
        super::Failure::Unreachable => (t!("store.search.unreachable", source = name), true),
        super::Failure::Unreadable => (t!("store.search.unreadable", source = name), true),
    };
    let glyph = ui.env().icons().glyph("warning").into_owned();
    ui.row(|ui| {
        ui.add(Text::new(format!("{glyph} {text}")).color("warning").no_wrap());
        if retry {
            ui.add(Button::new(t!("store.search.retry")).on_press(Msg::Retry(source)))
                .id(format!("store-retry-{}", crate::sources::name(source)));
        }
    })
    .gap(2)
    .fill_width();
}

/// Where the grid's remembered card is in `cards`.
fn selected(store: &Store, grid: Grid, cards: &[Card]) -> Option<usize> {
    let key = store.selected.get(&grid)?;
    cards.iter().position(|card| card.key == *key)
}

/// Which of `cards` are checked.
fn checked(store: &Store, cards: &[Card]) -> Vec<bool> {
    cards.iter().map(|card| store.checked.iter().any(|(key, _)| *key == card.key)).collect()
}

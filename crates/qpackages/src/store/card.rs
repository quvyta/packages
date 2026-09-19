//! A grid of application cards: icon and name, the summary, and where it comes from.
//!
//! Cards are drawn while the grid paints, outside the view, where the language files cannot be
//! reached; every word a card shows is therefore worked out in the view first and handed to the
//! grid with the cards.

use std::rc::Rc;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::widgets::{CardGrid, EmptyState};
use qpackages_core::sources::Source;

use super::model::{Card, Installed, compact};
use super::{Grid, Msg};
use crate::icons;

/// Narrowest and widest a card gets, in cells.
pub const CARD_MIN: u16 = 24;
pub const CARD_MAX: u16 = 32;

/// Cells between two cards of a row.
pub const GAP: u16 = 2;

/// How much a card shows, from the width it has (design 3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Name, summary, sources.
    Full,
    /// Name and sources: under 60 columns the summary goes.
    NoSummary,
    /// Only the name: under 40 columns the source labels go too, since a single letter says
    /// nothing.
    NameOnly,
}

impl Detail {
    /// What a screen `width` columns wide has room for.
    pub const fn for_width(width: u16) -> Self {
        match width {
            0..40 => Self::NameOnly,
            40..60 => Self::NoSummary,
            _ => Self::Full,
        }
    }

    /// Rows of content a card takes.
    pub const fn rows(self) -> u16 {
        match self {
            Self::Full => 3,
            Self::NoSummary => 2,
            Self::NameOnly => 1,
        }
    }
}

/// How many cards fit side by side in `width` cells, the way the grid lays them out.
pub fn columns(width: u16) -> usize {
    if width <= CARD_MIN {
        return 1;
    }
    usize::from(((width + GAP) / (CARD_MIN + GAP)).max(1))
}

/// The words cards share, in the language on screen.
struct Words {
    turkish: bool,
    mode: GlyphMode,
    detail: Detail,
    /// Each card's source line, in the order of the cards.
    lines: Vec<String>,
    installed: String,
}

/// The source names as cards write them, in [`crate::sources::ALL`] order.
fn source_label(source: Source) -> String {
    t!(&format!("store.source.{}", crate::sources::name(source)))
}

/// The line under the summary: where the card comes from, joined by ` · `.
///
/// A card has about twenty cells for it, so the order puts what matters first and lets the end
/// be cut: an installed card names the source it is installed from, since that is the one that
/// counts now; an AUR card with votes leads with them, the way the design draws the AUR row;
/// every other card lists its sources most trusted first.
fn source_line(card: &Card, installed: &Installed) -> String {
    let offers = &card.app.offers;
    if card.installed {
        return offers
            .iter()
            .filter(|offer| installed.has(offer))
            .map(|offer| source_label(offer.source))
            .next()
            .unwrap_or_default();
    }
    let mut parts = Vec::new();
    // Votes speak for a card the AUR carries; on a repository application an AUR build that
    // joined it is the least of its sources.
    let votes = card.votes.filter(|_| card.app.offered_by(Source::Aur) && !card.app.offered_by(Source::Pacman));
    if let Some(votes) = votes {
        parts.push(source_label(Source::Aur));
        parts.push(t!("store.card.votes", n = votes_arg(votes), count = compact(votes)));
    }
    let rest = offers.iter().filter(|offer| votes.is_none() || offer.source != Source::Aur);
    parts.extend(rest.map(|offer| source_label(offer.source)));
    parts.join(" · ")
}

/// The vote count as the plural rule's number.
fn votes_arg(votes: u64) -> i64 {
    i64::try_from(votes).unwrap_or(i64::MAX)
}

/// A grid of `cards`, `installed` naming the installed packages: selected, checked and opened through `grid`'s messages. `empty` is shown
/// when there are no cards; without it an empty grid draws nothing.
pub fn grid(
    ui: &mut View<'_, Msg>,
    installed: &Installed,
    grid: Grid,
    cards: &Rc<[Card]>,
    selected: Option<usize>,
    checked: Vec<bool>,
    empty: Option<EmptyState<Msg>>,
) -> CardGrid<Msg> {
    let detail = Detail::for_width(ui.size().width);
    let words = Rc::new(Words {
        turkish: ui.env().i18n().active().starts_with("tr"),
        mode: ui.env().icons().mode(),
        detail,
        lines: if detail == Detail::NameOnly {
            Vec::new()
        } else {
            cards.iter().map(|card| source_line(card, installed)).collect()
        },
        installed: format!("{} {}", t!("store.card.installed"), ui.env().icons().glyph("check")),
    });
    let shared = Rc::clone(cards);
    let mut built = CardGrid::new(cards.len())
        .card_width(CARD_MIN, CARD_MAX)
        .card_height(detail.rows())
        .gap(GAP, 1)
        .selected(selected)
        .checked(checked)
        .on_select(move |index| Msg::Select(grid, index))
        .on_activate(move |index| Msg::Open(grid, index))
        .on_toggle(move |index| Msg::Toggle(grid, index))
        .card(move |ui, index| draw(ui, &shared[index], index, selected == Some(index), &words));
    if let Some(empty) = empty {
        built = built.empty(empty);
    }
    built
}

/// One card's content.
fn draw(ui: &mut View<'_, Msg>, card: &Card, index: usize, selected: bool, words: &Words) {
    let icon = icons::glyph(card.icon_name(), card.app.category, card.first_source(), words.mode);
    // The icon is quiet beside the name, and joins the text's colour on the lit card.
    let icon_color = if selected { "text" } else { "muted" };
    let mut name = vec![Span::new(format!("{icon} ")).color(icon_color), Span::new(card.name(words.turkish)).bold()];
    if words.detail == Detail::NameOnly && card.installed {
        name.push(Span::new(format!(" {}", words.installed)).color("success"));
    }
    ui.add(Text::rich(name).no_wrap()).fill_width();
    if words.detail == Detail::Full {
        ui.add(Text::new(card.summary(words.turkish).unwrap_or_default()).color("muted").no_wrap()).fill_width();
    }
    if words.detail != Detail::NameOnly {
        let sources = &words.lines[index];
        if card.installed {
            // The mark and its word are never cut; where the card is too narrow for both, the
            // source's name gives way first.
            ui.row(|ui| {
                let separator = if sources.is_empty() { "" } else { " · " };
                if !sources.is_empty() {
                    ui.add(Text::new(sources.clone()).color("muted").no_wrap()).fill_width();
                }
                let width = qframe::text::width(separator) + qframe::text::width(&words.installed);
                let mark = [Span::new(separator).color("muted"), Span::new(words.installed.clone()).color("success")];
                ui.add(Text::rich(mark).no_wrap()).width(Length::Cells(width));
            });
        } else {
            ui.add(Text::new(sources.clone()).color("muted").no_wrap()).fill_width();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_shows_less_as_the_screen_narrows() {
        assert_eq!(Detail::for_width(96), Detail::Full);
        assert_eq!(Detail::for_width(59), Detail::NoSummary);
        assert_eq!(Detail::for_width(39), Detail::NameOnly);
        assert_eq!(Detail::NameOnly.rows(), 1);
    }

    #[test]
    fn columns_follow_the_design_formula() {
        // 96 columns less the kinds column and its gap leave 76: three cards of at least 24.
        assert_eq!(columns(96 - 18 - 2), 3);
        assert_eq!(columns(140 - 18 - 2), 4);
        assert_eq!(columns(20), 1);
    }
}

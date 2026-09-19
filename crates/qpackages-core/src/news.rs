//! Arch Linux's news, read before a system update.
//!
//! An unattended `pacman -Syu` has one well-known way to break a system: an update that needs the
//! user to do something by hand, which Arch announces in its news. The feed is fetched like the
//! catalog's sources, with `curl` run by the application ([`crate::catalog::net::curl_args`] on
//! [`NEWS_URL`]); this module reads the answer and keeps what was published lately.

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;

use crate::catalog::Problem;

/// The feed of Arch's news.
pub const NEWS_URL: &str = "https://archlinux.org/feeds/news/";

/// How far back news is shown before an update, in days.
pub const RECENT_DAYS: i64 = 14;

/// Seconds in a day.
const DAY: i64 = 86_400;

/// One news item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsItem {
    /// The headline, as Arch wrote it (in English).
    pub title: String,
    /// The article's address.
    pub link: String,
    /// When it was published, in seconds since the Unix epoch.
    pub published: i64,
    /// Whether the headline says the update needs the user's hand: Arch's own phrase for it is
    /// "manual intervention", so these are shown in a warning tone.
    pub manual_intervention: bool,
}

/// Reads the feed: every item that has a headline, an address and a readable date, newest first.
///
/// An item missing one of those is skipped: without a date it cannot be placed among the recent
/// ones, and a headline alone gives the user nothing to open.
///
/// # Errors
///
/// Returns where the text stops being XML.
pub fn parse_news(xml: &str) -> Result<Vec<NewsItem>, Problem> {
    let mut reader = Reader::from_str(xml);
    let mut items = Vec::new();
    let mut item: Option<Draft> = None;
    let mut field: Option<(Field, String)> = None;
    loop {
        let event = reader.read_event().map_err(|error| {
            Problem::at(xml, usize::try_from(reader.error_position()).unwrap_or(usize::MAX), error.to_string())
        })?;
        match event {
            Event::Start(tag) => match (tag.local_name().as_ref(), item.is_some()) {
                ("item", _) => item = Some(Draft::default()),
                ("title", true) => field = Some((Field::Title, String::new())),
                ("link", true) => field = Some((Field::Link, String::new())),
                ("pubDate", true) => field = Some((Field::Date, String::new())),
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, collected)) = field.as_mut() {
                    collected.push_str(&text.xml10_content());
                }
            }
            Event::CData(data) => {
                if let Some((_, collected)) = field.as_mut() {
                    collected.push_str(&data.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((_, collected)) = field.as_mut() {
                    let written = format!("&{};", reference.xml10_content());
                    // An entity XML does not define is kept as written rather than dropping the item.
                    collected.push_str(&unescape(&written).map_or(written.clone(), std::borrow::Cow::into_owned));
                }
            }
            Event::End(tag) => {
                if tag.local_name().as_ref() == "item" {
                    items.extend(item.take().and_then(Draft::finish));
                } else if let (Some((kind, text)), Some(draft)) = (field.take(), item.as_mut()) {
                    draft.take(kind, text.trim());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.published));
    Ok(items)
}

/// The items of `items` published in the last [`RECENT_DAYS`] days before `now`, in seconds
/// since the Unix epoch. An item dated after `now` is kept: the clock may simply be behind.
#[must_use]
pub fn recent(items: &[NewsItem], now: i64) -> Vec<NewsItem> {
    items.iter().filter(|item| item.published >= now - RECENT_DAYS * DAY).cloned().collect()
}

/// Which part of an item is being read.
#[derive(Debug, Clone, Copy)]
enum Field {
    Title,
    Link,
    Date,
}

/// An item as far as it has been read.
#[derive(Debug, Default)]
struct Draft {
    title: Option<String>,
    link: Option<String>,
    published: Option<i64>,
}

impl Draft {
    fn take(&mut self, field: Field, text: &str) {
        match field {
            Field::Title => self.title = Some(text.split_whitespace().collect::<Vec<_>>().join(" ")),
            Field::Link => self.link = Some(text.to_owned()),
            Field::Date => self.published = parse_rfc2822(text),
        }
    }

    fn finish(self) -> Option<NewsItem> {
        let (title, link, published) = (self.title?, self.link?, self.published?);
        if title.is_empty() || link.is_empty() {
            return None;
        }
        let manual_intervention = title.to_ascii_lowercase().contains("manual intervention");
        Some(NewsItem { title, link, published, manual_intervention })
    }
}

/// Reads an RSS date such as `Tue, 21 Jul 2026 13:01:46 +0000` into seconds since the Unix
/// epoch; `None` for anything else. The weekday is optional and not checked; the zone is a
/// numeric offset or `GMT`, `UT` or `Z`.
fn parse_rfc2822(text: &str) -> Option<i64> {
    let text = text.split_once(',').map_or(text, |(_, rest)| rest);
    let words: Vec<&str> = text.split_whitespace().collect();
    let [day, month, year, time, zone] = words.as_slice() else { return None };
    let day: i64 = number(day, 1, 31)?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|name| name == month)?;
    let year: i64 = number(year, 1970, 9999)?;
    let mut clock = time.split(':');
    let hour = number(clock.next()?, 0, 23)?;
    let minute = number(clock.next()?, 0, 59)?;
    let second = clock.next().map_or(Some(0), |second| number(second, 0, 60))?;
    if clock.next().is_some() {
        return None;
    }
    let offset = match *zone {
        "GMT" | "UT" | "UTC" | "Z" => 0,
        zone => {
            let (sign, digits) = match zone.split_at_checked(1)? {
                ("+", digits) => (1, digits),
                ("-", digits) => (-1, digits),
                _ => return None,
            };
            if digits.len() != 4 || !digits.is_ascii() {
                return None;
            }
            sign * (number(&digits[..2], 0, 23)? * 3600 + number(&digits[2..], 0, 59)? * 60)
        }
    };
    let month = i64::try_from(month).ok()? + 1;
    Some(days_from_civil(year, month, day) * DAY + hour * 3600 + minute * 60 + second - offset)
}

/// `text` as a number of plain digits between `low` and `high`.
fn number(text: &str, low: i64, high: i64) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|value| (low..=high).contains(value))
}

/// Days from 1970-01-01 to the given date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = include_str!("../tests/fixtures/arch-news.xml");

    /// 2026-07-25 00:00:00 UTC.
    const JULY_25: i64 = 1_784_937_600;

    #[test]
    fn dates_read_as_the_calendar_says() {
        assert_eq!(parse_rfc2822("Thu, 01 Jan 1970 00:00:00 +0000"), Some(0));
        assert_eq!(parse_rfc2822("Sat, 25 Jul 2026 00:00:00 GMT"), Some(JULY_25));
        assert_eq!(parse_rfc2822("25 Jul 2026 03:00:00 +0300"), Some(JULY_25), "a zone ahead of UTC");
        assert_eq!(parse_rfc2822("Fri, 24 Jul 2026 22:30:00 -0130"), Some(JULY_25), "a zone behind UTC");
        assert_eq!(parse_rfc2822("Tue, 29 Feb 2028 12:00:00 +0000"), Some(1_835_438_400), "a leap day");
    }

    #[test]
    fn a_date_that_does_not_read_is_none() {
        for text in [
            "",
            "yesterday",
            "Tue, 21 Jux 2026 13:01:46 +0000",
            "Tue, 32 Jul 2026 13:01:46 +0000",
            "Tue, 21 Jul 2026 25:01:46 +0000",
            "Tue, 21 Jul 2026 13:01:46",
            "Tue, 21 Jul 2026 13:01:46 +00",
            "Tue, 21 Jul 2026 13:01:46:00 +0000",
            "Tue, 21 Jul 2026 13:01:46 CEST",
            "Tue, -1 Jul 2026 13:01:46 +0000",
            "Tue, 21 Jul 2026 13:01:46 +0ü1",
        ] {
            assert_eq!(parse_rfc2822(text), None, "`{text}`");
        }
    }

    #[test]
    fn reads_the_recorded_feed_newest_first() {
        let items = parse_news(FEED).expect("the recorded feed reads");
        assert_eq!(items.len(), 10);
        assert_eq!(items[0].title, "virtualbox-ext-vnc >= 7.2.12-2 requires manual intervention");
        assert_eq!(items[0].link, "https://archlinux.org/news/virtualbox-ext-vnc-7212-2-requires-manual-intervention/");
        assert_eq!(items[0].published, parse_rfc2822("Tue, 21 Jul 2026 13:01:46 +0000").expect("a date"));
        assert!(items.windows(2).all(|pair| pair[0].published >= pair[1].published));
        let flagged: Vec<&str> =
            items.iter().filter(|item| item.manual_intervention).map(|item| item.title.as_str()).collect();
        assert_eq!(flagged.len(), 5, "{flagged:?}");
        assert!(!items[1].manual_intervention, "an incident notice is news, not a manual step");
    }

    #[test]
    fn only_the_last_fourteen_days_are_recent() {
        let items = parse_news(FEED).expect("the recorded feed reads");
        let recent = recent(&items, JULY_25);
        assert_eq!(recent.len(), 1);
        assert!(recent[0].manual_intervention);
        assert!(super::recent(&items, JULY_25 + 30 * DAY).is_empty(), "a month later nothing is recent");
        assert_eq!(super::recent(&items, 0).len(), items.len(), "a clock that is behind keeps everything");
    }

    #[test]
    fn an_item_without_a_date_or_an_address_is_skipped() {
        let xml = "<rss><channel><title>Feed</title>\
            <item><title>No date</title><link>https://archlinux.org/news/a/</link></item>\
            <item><title>No link</title><pubDate>Tue, 21 Jul 2026 13:01:46 +0000</pubDate></item>\
            <item><title>Bad date</title><link>https://x/</link><pubDate>soon</pubDate></item>\
            <item><title><![CDATA[Kept & read]]></title><link>https://archlinux.org/news/b/</link>\
            <pubDate>Tue, 21 Jul 2026 13:01:46 +0000</pubDate></item></channel></rss>";
        let items = parse_news(xml).expect("well-formed");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Kept & read", "the channel's own title is not an item");
    }

    #[test]
    fn broken_xml_says_where_and_never_panics() {
        let problem = parse_news("<rss>\n<channel><item><title>x</item>").expect_err("mismatched tags");
        assert_eq!(problem.line, 2);
        for cut in 0..FEED.len() {
            if FEED.is_char_boundary(cut) {
                let _ = parse_news(&FEED[..cut]);
            }
        }
    }
}

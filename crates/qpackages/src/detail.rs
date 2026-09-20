//! The detail panel: everything the local database knows about the selected package.

use qframe::prelude::*;
use qframe::widgets::{CopyValue, EmptyState, ScrollView};
use qpackages_core::pacman::Package;

use crate::icons;
use crate::installed::table::size_text;

/// Shows `package`, or a quiet empty state while nothing is selected.
pub fn view<Msg: Clone + 'static>(package: Option<&Package>, ui: &mut View<'_, Msg>) {
    let Some(package) = package else {
        ui.add(EmptyState::new(t!("detail.no-selection")).message(t!("detail.no-selection-message"))).fill();
        return;
    };
    // Keyed by name so the copy confirmation of one package never carries over to the next.
    ui.add_with(ScrollView::new(), |ui| {
        ui.column(|ui| {
            // The icon is quiet beside the name: it helps the eye find the package, the name says
            // what it is.
            let drawing = ui.env().icons();
            let icon = icons::installed(package, drawing.mode()).resolve(drawing).into_owned();
            ui.add(
                Text::rich([Span::new(format!("{icon} ")).color("muted"), Span::new(package.name.clone())])
                    .role("title")
                    .no_wrap(),
            );
            let description = package.description.clone().unwrap_or_else(|| t!("detail.no-description"));
            ui.add(Text::new(description)).fill_width();
            ui.column(|ui| {
                field(ui, &t!("detail.version"), package.version.clone());
                let reason =
                    if package.explicit { t!("detail.reason-explicit") } else { t!("detail.reason-dependency") };
                field(ui, &t!("detail.reason"), reason);
                if let Some(size) = package.size {
                    field(ui, &t!("detail.size"), size_text(size));
                }
                if let Some(installed) = package.installed {
                    field(ui, &t!("detail.installed"), date_text(installed));
                }
                if !package.licenses.is_empty() {
                    field(ui, &t!("detail.licenses"), package.licenses.join(", "));
                }
            })
            .fill_width();
            dependencies(ui, package);
            if let Some(url) = &package.url {
                ui.column(|ui| {
                    ui.add(Text::new(t!("detail.url")).role("faint").no_wrap());
                    ui.add(CopyValue::new(url.clone())).id("url");
                })
                .fill_width();
            }
        })
        .gap(1)
        .fill_width()
        .id(package.name.clone());
    })
    .fill();
}

/// A label and its value, the label faint; a long value wraps under it rather than being cut.
fn field<Msg: 'static>(ui: &mut View<'_, Msg>, label: &str, value: String) {
    ui.add(Text::rich([Span::new(format!("{label}  ")).role("faint"), Span::new(value)])).fill_width();
}

fn dependencies<Msg: 'static>(ui: &mut View<'_, Msg>, package: &Package) {
    ui.column(|ui| {
        if package.depends.is_empty() {
            ui.add(Text::new(t!("detail.no-depends")).role("faint").no_wrap());
            return;
        }
        ui.add(Text::new(t!("detail.depends")).role("faint").no_wrap());
        let items = package.depends.iter().map(|dependency| ListItem::new(dependency.clone()));
        ui.add(List::new(items)).fill_width().height(Length::Cells(clamp_rows(package.depends.len())));
    })
    .fill_width();
}

/// Rows the dependency list takes: one per dependency, at most ten before it scrolls inside.
fn clamp_rows(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX).min(10)
}

/// A Unix timestamp as `YYYY-MM-DD` in UTC. Install dates are a day at most off from local
/// time, which is close enough for "when did this arrive"; a date crate for that alone is not
/// worth its weight.
#[must_use]
pub fn date_text(unix: i64) -> String {
    let (year, month, day) = civil_from_days(unix.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// A Unix timestamp as `YYYY-MM-DD HH:MM` where the machine stands, for moments where the hour
/// matters, such as when a lock was taken. Falls back to UTC where the system's time zone is
/// unknown, which the framework reports in the same way.
#[must_use]
pub fn date_time_text(unix: i64) -> String {
    date_time_at(unix, qframe::date::local_offset_minutes())
}

/// [`date_time_text`] in an explicit offset from UTC, in minutes.
fn date_time_at(unix: i64, offset_minutes: i16) -> String {
    let moment = qframe::date::DateTime::from_unix(unix, offset_minutes);
    let (date, time) = (moment.date, moment.time);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", date.year(), date.month(), date.day(), time.hour, time.minute)
}

/// Days since 1970-01-01 to a proleptic Gregorian date, by the well-known era arithmetic.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // Both fit: a month is 1..=12 and a day 1..=31 by construction.
    (year, u32::try_from(month).unwrap_or(1), u32::try_from(day).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_come_out_as_the_calendar_says() {
        assert_eq!(date_text(0), "1970-01-01");
        assert_eq!(date_text(-1), "1969-12-31");
        assert_eq!(date_text(951_782_400), "2000-02-29", "a leap day in a century year");
        assert_eq!(date_text(1_781_367_195), "2026-06-13", "the fixture's install date");
        assert_eq!(date_text(4_102_444_800), "2100-01-01");
    }

    #[test]
    fn moments_are_written_in_the_offset_they_are_given() {
        assert_eq!(date_time_at(1_773_997_200, 0), "2026-03-20 09:00");
        assert_eq!(date_time_at(1_773_997_200, 180), "2026-03-20 12:00", "noon in Istanbul");
        assert_eq!(date_time_at(0, -60), "1969-12-31 23:00", "an offset can cross midnight");
        assert_eq!(date_time_text(1_773_997_200).len(), "2026-03-20 09:00".len());
    }
}

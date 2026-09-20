//! What the review screen puts on the page.
//!
//! The shape follows the recipes themselves: what the rules point at on top, because it is the
//! reason to look; the files at the left, because a recipe is a handful of them; the code at the
//! right, as a difference where there is one to show and in full where there is not. The sentence
//! above the buttons says what the check is and is not, so a clean screen is never read as a
//! promise.

use qframe::prelude::*;
use qframe::widgets::{CodeView, EmptyState, Language, LineMark, LineTone, ScrollView, Select, Spinner};
use qpackages_core::review::diff::{Kind, Line};
use qpackages_core::review::{Change, Finding};

use super::{Base, Msg, Progress, Screen, status_text};

/// Cells the file list takes beside the code.
const FILES_WIDTH: u16 = 26;

/// Below this width the file list becomes a one-line choice above the code.
const NARROW: u16 = 80;

/// Rows the list of findings takes before it scrolls inside itself.
const MAX_FINDING_ROWS: u16 = 5;

/// Draws the screen.
pub fn view(screen: &Screen, ui: &mut View<'_, Msg>) {
    ui.column(|ui| match &screen.progress {
        Progress::Reading => reading(ui),
        Progress::Failed(trouble) => {
            let message = t!(&format!("review.trouble.{}", trouble.key), base = trouble.base.clone());
            ui.add(
                EmptyState::new(t!("review.failed"))
                    .message(format!("{message}\n{}", trouble.detail))
                    .action(Button::new(t!("review.back")).on_press(Msg::Cancel)),
            )
            .fill();
        }
        Progress::Read(bases) => read(screen, bases, ui),
    })
    .gap(1)
    .fill();
}

/// While the recipes are being fetched: one line, so the screen does not jump when they arrive.
fn reading(ui: &mut View<'_, Msg>) {
    ui.column(|ui| {
        ui.row(|ui| {
            ui.add(Spinner::new());
            ui.add(Text::new(t!("review.reading")));
        })
        .gap(1);
    })
    .fill();
}

/// The screen once every recipe was read.
fn read(screen: &Screen, bases: &[Base], ui: &mut View<'_, Msg>) {
    let narrow = ui.size().width < NARROW;
    ui.add(Text::new(t!("review.title")).role("title").no_wrap());
    ui.add(Text::new(lead(bases)).role("secondary")).fill_width();
    // One line per base, above everything its files are in: how its recipe compares is the first
    // thing to know, and for a recipe that needs no reading it is the whole answer.
    ui.column(|ui| {
        for base in bases {
            ui.add(Text::rich([
                Span::new(format!("aur/{}  ", base.wanted.base)),
                Span::new(status_text(base.checked.status)).role("faint"),
            ]))
            .fill_width();
        }
    })
    .fill_width();
    findings(screen, ui);
    if narrow {
        file_choice(screen, ui);
    }
    ui.row(|ui| {
        if !narrow {
            ui.column(|ui| files(screen, ui)).width(Length::Cells(FILES_WIDTH)).fill_height();
        }
        ui.column(|ui| code(screen, ui)).fill();
    })
    .gap(2)
    .fill();
    ui.add(Text::new(t!("review.disclaimer")).color("warning")).fill_width();
    ui.row(|ui| {
        ui.add(Button::new(t!("review.cancel")).on_press(Msg::Cancel)).id("review-cancel");
        ui.add(Button::new(t!("review.approve")).variant("primary").on_press(Msg::Approve)).id("review-approve");
    })
    .gap(2)
    .align(Align::End)
    .fill_width();
}

/// The line under the title: how many recipes are to be read, or that none is.
fn lead(bases: &[Base]) -> String {
    let reading = bases.iter().filter(|base| base.needs_reading()).count();
    if reading == 0 { t!("review.nothing-to-read") } else { t!("review.lead", n = reading) }
}

/// What the rules point at, over the code: one row each, and the reason of the chosen one under
/// them. A screen with nothing to point at says so in one line, since an empty pane would read
/// as a promise.
fn findings(screen: &Screen, ui: &mut View<'_, Msg>) {
    let found = screen.findings();
    if found.is_empty() {
        ui.add(Text::new(t!("review.no-findings")).role("secondary")).fill_width();
        return;
    }
    ui.add_with(Panel::new().title(t!("review.findings", n = found.len())), |ui| {
        let items = found.iter().map(|(base, finding)| {
            let where_at = place_text(screen, *base, finding);
            ListItem::new(t!(&format!("review.pattern.{}", finding.rule.key())))
                .icon("warning", Some("warning"))
                .detail(where_at)
        });
        let rows = u16::try_from(found.len()).unwrap_or(MAX_FINDING_ROWS).min(MAX_FINDING_ROWS);
        ui.add(List::new(items).selected(screen.at).on_select(Msg::Finding).on_activate(Msg::Finding))
            .fill_width()
            .height(Length::Cells(rows))
            .id("review-findings");
        let reason = screen
            .at
            .and_then(|index| found.get(index))
            .map(|(_, finding)| t!(&format!("review.rule.{}", finding.rule.key()), host = finding.detail.clone()));
        ui.add(Text::new(reason.unwrap_or_else(|| t!("review.pick-a-finding"))).role("secondary")).fill_width();
    })
    .fill_width();
}

/// Where a finding is, as its row's right-hand detail: the file and the line, under the base's
/// name when the build has several. A finding about the package rather than a line, such as a new
/// maintainer, names no place.
fn place_text(screen: &Screen, base: usize, finding: &Finding) -> String {
    let name = screen.bases().get(base).map_or_else(String::new, |base| base.wanted.base.clone());
    let place = finding.line.map(|line| format!("{}:{line}", finding.file)).unwrap_or_default();
    if screen.bases().len() == 1 {
        return place;
    }
    format!("{name}  {place}").trim_end().to_owned()
}

/// The files at the left: every file of a recipe that has to be read, with how many of its lines
/// changed; a file nothing touched is faint.
fn files(screen: &Screen, ui: &mut View<'_, Msg>) {
    let spots = screen.spots();
    let items = spots.iter().map(|(index, name)| {
        let base = &screen.bases()[*index];
        let item = ListItem::new(file_label(screen, *index, name));
        match base.changed_lines(name) {
            Some(lines) => item.detail(t!("review.changed-lines", n = lines)),
            None => item.faint(true),
        }
    });
    let chosen = chosen_file(screen, &spots);
    ui.add(List::new(items).selected(chosen).on_select(Msg::File).on_activate(Msg::File)).fill().id("review-files");
}

/// On a narrow screen the files are a one-line choice instead of a column.
fn file_choice(screen: &Screen, ui: &mut View<'_, Msg>) {
    let spots = screen.spots();
    if spots.is_empty() {
        return;
    }
    let names = spots.iter().map(|(index, name)| file_label(screen, *index, name));
    let chosen = chosen_file(screen, &spots);
    ui.add(Select::new(names).selected(chosen).on_select(Msg::File)).fill_width().id("review-file-choice");
}

/// How a file is named in the list: by itself for a single base, under its base for a build of
/// several.
fn file_label(screen: &Screen, index: usize, name: &str) -> String {
    if screen.bases().len() == 1 {
        return name.to_owned();
    }
    format!("{}/{name}", screen.bases()[index].wanted.base)
}

/// Where the shown file is among `spots`.
fn chosen_file(screen: &Screen, spots: &[(usize, String)]) -> Option<usize> {
    let shown = screen.shown.as_ref()?;
    spots.iter().position(|spot| spot == shown)
}

/// The code at the right: the chosen file as a difference where there is one, in full otherwise.
fn code(screen: &Screen, ui: &mut View<'_, Msg>) {
    let Some((index, name)) = &screen.shown else {
        ui.add(EmptyState::new(t!("review.nothing-open")).message(t!("review.nothing-open-message"))).fill();
        return;
    };
    let Some(base) = screen.bases().get(*index) else { return };
    let shown = Shown::of(base, name);
    let changed = shown.marks.iter().any(|mark| *mark != LineMark::Unchanged);
    let mut code = CodeView::new(shown.text.clone(), Language::from_file_name(name));
    if changed {
        // A difference holds both versions' lines, so a row is not a line of either file: the
        // numbers would name the wrong lines, and the marks say what changed instead.
        code = code.line_marks(shown.marks.clone()).line_numbers(false);
    }
    for row in base
        .checked
        .findings
        .iter()
        .filter(|finding| finding.file == *name)
        .filter_map(|finding| finding.line.and_then(|line| shown.row_of(line)))
    {
        code = code.highlight_lines(row..=row, LineTone::Warning);
    }
    // The line gone to takes the accent tone over its finding's warning, and the view scrolls to
    // it once; scrolling away afterwards is the user's.
    if let Some(row) = screen.at.and_then(|at| went_to(screen, *index, at)).and_then(|line| shown.row_of(line)) {
        code = code.reveal(row).highlight_lines(row..=row, LineTone::Accent);
    }
    ui.add_with(ScrollView::new(), |ui| {
        ui.add(code).fill_width().id(format!("{}/{name}", base.wanted.base));
    })
    .fill()
    .id("review-code");
}

/// The line the chosen finding points at, when it belongs to base `index` and names a line.
fn went_to(screen: &Screen, index: usize, at: usize) -> Option<usize> {
    let (base, finding) = screen.findings().get(at).copied()?;
    (base == index).then_some(finding.line).flatten()
}

/// One file as the code view shows it: every row's text, its difference mark, and which line of
/// the new recipe it is, so a finding's line finds its row.
struct Shown {
    text: String,
    marks: Vec<LineMark>,
    /// The line of the fetched recipe each row stands for; `None` for a removed line.
    lines: Vec<Option<usize>>,
}

impl Shown {
    /// How `name` of `base` is shown.
    fn of(base: &Base, name: &str) -> Self {
        if let Change::Changed(files) = &base.checked.review.change
            && let Some(diff) = files.iter().find(|diff| diff.name == name)
        {
            return Self::of_diff(&diff.lines);
        }
        let text = base.recipe.files.iter().find(|file| file.name == name).map_or("", |file| file.text.as_str());
        let count = text.lines().count();
        Self { text: text.to_owned(), marks: vec![LineMark::Unchanged; count], lines: (1..=count).map(Some).collect() }
    }

    /// A difference: every line of both versions, in order, each marked.
    fn of_diff(lines: &[Line]) -> Self {
        Self {
            text: lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n"),
            marks: lines
                .iter()
                .map(|line| match line.kind {
                    Kind::Same => LineMark::Unchanged,
                    Kind::Added => LineMark::Added,
                    Kind::Removed => LineMark::Removed,
                })
                .collect(),
            lines: lines.iter().map(|line| line.new).collect(),
        }
    }

    /// The row, counted from 1, that shows line `line` of the fetched recipe.
    fn row_of(&self, line: usize) -> Option<usize> {
        self.lines.iter().position(|at| *at == Some(line)).map(|index| index + 1)
    }
}

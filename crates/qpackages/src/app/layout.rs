//! How the screen is divided, computed from the screen size alone so that `view`, which lays the
//! parts out, and `update`, which starts pacman on a terminal as wide as its output pane, never
//! disagree.

use qframe::prelude::Size;

/// The fewest rows the page keeps above the output pane, and the pane keeps below them.
pub const MIN_PACKAGES: u16 = 4;
pub const MIN_OUTPUT: u16 = 4;

/// Rows the shell's header and footer take around the body: the tabs and the key hints.
const FRAME_ROWS: u16 = 2;

/// The row the splitter's handle takes between the packages and the output pane.
const HANDLE: u16 = 1;

/// Rows of the output pane that are not pacman's lines: its heading with the progress bar.
const PANE_HEADING: u16 = 1;

/// Columns of the output pane that pacman's text never reaches, so its progress bars end where
/// the pane does: the pane's padding on both sides (2), then what the log view draws before each
/// line, which is the selection pillar with its space (2), the level word (5) and the gap after it
/// (2), plus the spare cell it keeps at the right edge (1) and the scrollbar column once the
/// output has grown past the pane (1).
const PANE_CHROME: u16 = 13;

/// The narrowest terminal pacman is given, whatever the screen: below this its progress bars
/// are unreadable anyway, and a size of zero would be refused.
const MIN_PTY: (u16, u16) = (20, 3);

/// Where the body's parts fall on a screen of a given size while the output pane is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLayout {
    /// Rows the packages get above the handle.
    pub packages: u16,
    /// The most rows the packages may be dragged to while the pane keeps its minimum.
    pub widest: u16,
    /// Rows the body has for the packages, the handle and the pane together.
    pub body_rows: u16,
    /// The terminal pacman draws on: as many columns as the pane shows of each line, as many
    /// rows as the pane shows lines.
    pub pty: (u16, u16),
}

/// Lays the body out for a `size` screen with the output pane dragged to `output_height` rows.
#[must_use]
pub fn output_layout(size: Size, output_height: u16) -> OutputLayout {
    let body_rows = size.height.saturating_sub(FRAME_ROWS);
    // The body is as wide as the screen: there is no sidebar beside it.
    let body_width = size.width;
    let widest = body_rows.saturating_sub(MIN_OUTPUT + HANDLE).max(MIN_PACKAGES);
    let wanted = body_rows.saturating_sub(output_height + HANDLE);
    // The splitter itself keeps its handle and one cell of the pane whatever the limits say.
    let packages = wanted.clamp(MIN_PACKAGES, widest).min(body_rows.saturating_sub(HANDLE + 1)).min(body_rows);
    let pane_rows = body_rows.saturating_sub(packages + HANDLE);
    let pty =
        (body_width.saturating_sub(PANE_CHROME).max(MIN_PTY.0), pane_rows.saturating_sub(PANE_HEADING).max(MIN_PTY.1));
    OutputLayout { packages, widest, body_rows, pty }
}

impl OutputLayout {
    /// The rows the pane gets when the packages are dragged to `packages` rows.
    #[must_use]
    pub fn pane_rows_for(&self, packages: u16) -> u16 {
        self.body_rows.saturating_sub(packages + HANDLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pane_gets_the_rows_it_was_dragged_to_and_pacman_the_columns_it_shows() {
        let layout = output_layout(Size::new(120, 30), 10);
        assert_eq!(layout.body_rows, 28);
        assert_eq!(layout.packages, 17, "28 body rows less 10 for the pane and 1 for the handle");
        assert_eq!(layout.pane_rows_for(layout.packages), 10);
        assert_eq!(layout.pty, (107, 9), "120 body columns less the pane's chrome, 10 rows less the heading");
        assert_eq!(output_layout(Size::new(60, 20), 10).pty, (47, 9));
    }

    #[test]
    fn a_pane_dragged_too_far_leaves_the_packages_their_minimum() {
        let layout = output_layout(Size::new(120, 30), 100);
        assert_eq!(layout.packages, MIN_PACKAGES);
        assert_eq!(layout.pty, (107, 22));
        let layout = output_layout(Size::new(120, 30), 0);
        assert_eq!(layout.packages, layout.widest);
        assert_eq!(layout.pty.1, MIN_OUTPUT.saturating_sub(PANE_HEADING).max(MIN_PTY.1));
    }

    #[test]
    fn a_tiny_screen_never_panics_and_keeps_the_terminal_usable() {
        for (width, height) in [(30, 8), (0, 0), (1, 1), (20, 3), (200, 4)] {
            let layout = output_layout(Size::new(width, height), 10);
            assert!(layout.pty.0 >= MIN_PTY.0 && layout.pty.1 >= MIN_PTY.1, "{width}x{height}: {layout:?}");
            assert!(layout.packages <= layout.body_rows, "{width}x{height}: {layout:?}");
        }
        assert_eq!(output_layout(Size::new(30, 8), 10).pty, MIN_PTY);
    }
}

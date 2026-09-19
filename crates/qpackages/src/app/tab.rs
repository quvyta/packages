//! The tabs of the header, in order. A new tab is a variant here, a place in [`TABS`] and its
//! label; the keys that open tabs by number and the arrows that step between them follow the
//! list, so the first tab is always `ctrl+1`.

use qframe::prelude::*;

/// A page the header's tabs open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The store: what can be installed, from every source.
    Discover,
    /// What is on this machine.
    Installed,
    /// What has a newer version.
    Updates,
}

/// The tabs, left to right. The first one is open when qpac starts.
pub const TABS: [Tab; 3] = [Tab::Discover, Tab::Installed, Tab::Updates];

impl Tab {
    /// The tab's label in the active language.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Discover => t!("tabs.discover"),
            Self::Installed => t!("tabs.installed"),
            Self::Updates => t!("tabs.updates"),
        }
    }

    /// Where the tab stands in [`TABS`].
    #[must_use]
    pub fn index(self) -> usize {
        TABS.iter().position(|tab| *tab == self).unwrap_or(0)
    }

    /// The key its page keeps its widgets' state under while another tab is open.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Installed => "installed",
            Self::Updates => "updates",
        }
    }
}

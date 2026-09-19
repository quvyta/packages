//! The kinds of software the store sorts by, and how a package finds its kind.
//!
//! The kind comes from the best source there is: the AppStream categories first, then the
//! pacman groups, then the name. Each step answers only when it is sure, so a package that none of
//! them places stays [`Category::Unknown`] rather than landing somewhere wrong.

/// A kind of software, one per card and per entry in the store's side column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Browsers, mail, chat, downloads.
    Internet,
    /// Players, editors and recorders for sound and video.
    AudioVideo,
    /// Image editors, viewers, drawing and 3D.
    Graphics,
    /// Documents, spreadsheets, notes, finance.
    Office,
    /// Games and their launchers.
    Game,
    /// Editors, compilers, debuggers and other tools for programmers.
    Development,
    /// Settings, monitors, drivers and the system itself.
    System,
    /// Small tools that fit nowhere else: archivers, terminals, calculators.
    Utility,
    /// Font families.
    Fonts,
    /// Code other programs use, not something a person opens.
    Library,
    /// Nothing placed it.
    Unknown,
}

impl Category {
    /// The kinds a person browses by, in the order of the store's side column.
    pub const BROWSABLE: [Self; 9] = [
        Self::Internet,
        Self::AudioVideo,
        Self::Graphics,
        Self::Office,
        Self::Game,
        Self::Development,
        Self::System,
        Self::Utility,
        Self::Fonts,
    ];

    /// A stable key for language files and icon names (`category.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Internet => "internet",
            Self::AudioVideo => "audio-video",
            Self::Graphics => "graphics",
            Self::Office => "office",
            Self::Game => "game",
            Self::Development => "development",
            Self::System => "system",
            Self::Utility => "utility",
            Self::Fonts => "fonts",
            Self::Library => "library",
            Self::Unknown => "unknown",
        }
    }

    /// The kind a key names; the inverse of [`Category::key`].
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Library, Self::Unknown].into_iter().chain(Self::BROWSABLE).find(|category| category.key() == key)
    }

    /// The kind the freedesktop categories of an application point to.
    ///
    /// Categories are tried in the order they are written, the way a desktop entry lists its
    /// main category first. Only the freedesktop main categories decide; additional ones such as
    /// `WebBrowser` always come with their main one. `Education` and `Science` have no kind of
    /// their own here and are passed over, so the next category decides.
    #[must_use]
    pub fn from_freedesktop<S: AsRef<str>>(categories: &[S]) -> Option<Self> {
        categories.iter().find_map(|category| match category.as_ref() {
            "Network" => Some(Self::Internet),
            "AudioVideo" | "Audio" | "Video" => Some(Self::AudioVideo),
            "Graphics" => Some(Self::Graphics),
            "Office" => Some(Self::Office),
            "Game" => Some(Self::Game),
            "Development" => Some(Self::Development),
            "System" | "Settings" => Some(Self::System),
            "Utility" => Some(Self::Utility),
            _ => None,
        })
    }

    /// The kind Flathub's `main_categories` value names. Flathub writes the freedesktop names in
    /// lower case.
    #[must_use]
    pub fn from_flathub(main_category: &str) -> Option<Self> {
        match main_category.to_ascii_lowercase().as_str() {
            "network" => Some(Self::Internet),
            "audiovideo" | "audio" | "video" => Some(Self::AudioVideo),
            "graphics" => Some(Self::Graphics),
            "office" => Some(Self::Office),
            "game" => Some(Self::Game),
            "development" => Some(Self::Development),
            "system" | "settings" => Some(Self::System),
            "utility" => Some(Self::Utility),
            _ => None,
        }
    }

    /// The kind a pacman group places its packages in.
    ///
    /// Desktop groups such as `gnome`, `plasma` or `kde-applications` hold every kind of program
    /// and place nothing. Groups of plugins go with what they plug into.
    #[must_use]
    pub fn from_group(group: &str) -> Option<Self> {
        let category = match group {
            "nerd-fonts" | "xorg-fonts" => Self::Fonts,
            "pro-audio" | "lv2-plugins" | "vst-plugins" | "vst3-plugins" | "clap-plugins" | "ladspa-plugins"
            | "dssi-plugins" | "gst-plugins-rs" | "kodi-addons" | "kde-multimedia" => Self::AudioVideo,
            "kde-games" | "libretro" => Self::Game,
            "kde-graphics" => Self::Graphics,
            "kde-network" | "firefox-addons" => Self::Internet,
            "kde-pim" => Self::Office,
            "base-devel"
            | "kde-sdk"
            | "vulkan-devel"
            | "dlang"
            | "mingw-w64"
            | "mingw-w64-toolchain"
            | "tree-sitter-grammars"
            | "vim-plugins"
            | "python-build-backend" => Self::Development,
            "kde-system" | "linux-tools" | "xorg-drivers" | "xorg" | "realtime" | "alpm" => Self::System,
            "kde-utilities" | "xfce4-goodies" => Self::Utility,
            "qt5" | "qt6" | "kf5" | "kf6" | "pyqt6" => Self::Library,
            _ if group.ends_with("-fonts") => Self::Fonts,
            _ => return None,
        };
        Some(category)
    }

    /// The kind a package's name gives away, when it does.
    ///
    /// `lib*` names are libraries, except `libre*`, which are programs (LibreOffice, LibreWolf).
    /// `lib32-*` builds and the `python-`, `perl-`, `ruby-`, `haskell-`, `ocaml-` and `lua-`
    /// modules are libraries too. `ttf-*`, `otf-*`, `noto-fonts*` and names ending in `-font` or
    /// `-fonts` are fonts. A `-bin` or `-git` ending does not change the answer.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        const LIBRARY_PREFIXES: [&str; 7] = ["lib32-", "python-", "perl-", "ruby-", "haskell-", "ocaml-", "lua-"];
        const FONT_PREFIXES: [&str; 3] = ["ttf-", "otf-", "noto-fonts"];
        let name = name.strip_suffix("-bin").or_else(|| name.strip_suffix("-git")).unwrap_or(name);
        if FONT_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
            || name.ends_with("-fonts")
            || name.ends_with("-font")
        {
            return Some(Self::Fonts);
        }
        if LIBRARY_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
            || (name.starts_with("lib") && !name.starts_with("libre"))
        {
            return Some(Self::Library);
        }
        None
    }

    /// The kind of a package from everything known about it, best source first: AppStream
    /// categories, then pacman groups, then the name. [`Category::Unknown`] when none of them is
    /// sure.
    #[must_use]
    pub fn classify<S: AsRef<str>, G: AsRef<str>>(categories: &[S], groups: &[G], name: &str) -> Self {
        Self::from_freedesktop(categories)
            .or_else(|| groups.iter().find_map(|group| Self::from_group(group.as_ref())))
            .or_else(|| Self::from_name(name))
            .unwrap_or(Self::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [&str; 0] = [];

    #[test]
    fn the_first_main_category_decides() {
        assert_eq!(Category::from_freedesktop(&["Network", "WebBrowser"]), Some(Category::Internet));
        assert_eq!(Category::from_freedesktop(&["AudioVideo", "Recorder"]), Some(Category::AudioVideo));
        assert_eq!(Category::from_freedesktop(&["Development", "Utility"]), Some(Category::Development));
        assert_eq!(Category::from_freedesktop(&["Settings", "HardwareSettings"]), Some(Category::System));
        assert_eq!(Category::from_freedesktop(&["Education", "Science", "Game"]), Some(Category::Game));
        assert_eq!(Category::from_freedesktop(&["Education", "Astronomy"]), None);
        assert_eq!(Category::from_freedesktop(&NONE), None);
    }

    #[test]
    fn flathub_names_are_the_same_in_lower_case() {
        assert_eq!(Category::from_flathub("network"), Some(Category::Internet));
        assert_eq!(Category::from_flathub("audiovideo"), Some(Category::AudioVideo));
        assert_eq!(Category::from_flathub("Game"), Some(Category::Game));
        assert_eq!(Category::from_flathub("science"), None);
    }

    #[test]
    fn groups_place_only_what_they_are_sure_of() {
        assert_eq!(Category::from_group("xorg-fonts"), Some(Category::Fonts));
        assert_eq!(Category::from_group("adobe-source-han-sans-fonts"), Some(Category::Fonts));
        assert_eq!(Category::from_group("kde-games"), Some(Category::Game));
        assert_eq!(Category::from_group("pro-audio"), Some(Category::AudioVideo));
        assert_eq!(Category::from_group("qt6"), Some(Category::Library));
        assert_eq!(Category::from_group("gnome"), None, "a desktop group holds every kind");
        assert_eq!(Category::from_group("kde-applications"), None);
    }

    #[test]
    fn names_give_away_libraries_and_fonts() {
        assert_eq!(Category::from_name("libxml2"), Some(Category::Library));
        assert_eq!(Category::from_name("lib32-mesa"), Some(Category::Library));
        assert_eq!(Category::from_name("python-requests"), Some(Category::Library));
        assert_eq!(Category::from_name("ttf-jetbrains-mono"), Some(Category::Fonts));
        assert_eq!(Category::from_name("otf-font-awesome"), Some(Category::Fonts));
        assert_eq!(Category::from_name("noto-fonts-emoji"), Some(Category::Fonts));
        assert_eq!(Category::from_name("terminus-font"), Some(Category::Fonts));
        assert_eq!(Category::from_name("ttf-ms-fonts-bin"), Some(Category::Fonts));
        assert_eq!(Category::from_name("libreoffice-fresh"), None, "LibreOffice is a program");
        assert_eq!(Category::from_name("librewolf-bin"), None);
        assert_eq!(Category::from_name("firefox"), None);
    }

    #[test]
    fn classification_takes_the_best_source_there_is() {
        assert_eq!(Category::classify(&["Graphics"], &["kde-applications"], "libfoo"), Category::Graphics);
        assert_eq!(Category::classify(&NONE, &["gnome", "kde-games"], "libfoo"), Category::Game);
        assert_eq!(Category::classify(&NONE, &NONE, "libfoo"), Category::Library);
        assert_eq!(Category::classify(&NONE, &NONE, "obs-vkcapture"), Category::Unknown);
    }

    #[test]
    fn every_key_reads_back() {
        for category in Category::BROWSABLE.into_iter().chain([Category::Library, Category::Unknown]) {
            assert_eq!(Category::from_key(category.key()), Some(category));
        }
        assert_eq!(Category::from_key("nonsense"), None);
    }
}

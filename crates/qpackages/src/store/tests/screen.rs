//! The home page and the application page as they are drawn.

use super::*;

/// The icon a card draws for `name` of `kind`, as `icons` draws it.
fn icon(name: &str, kind: qpackages_core::catalog::category::Category, icons: &qframe::icons::Icons) -> String {
    crate::icons::glyph(name, kind, Source::Pacman, icons.mode()).resolve(icons).into_owned()
}

#[test]
fn the_first_frame_shows_the_starter_list_before_anything_is_read() {
    let recorded = Arc::new(Recorded::default());
    let mut unread = page(&recorded, true);
    unread.init = false;
    let h = harness(unread, 96, 30, GlyphMode::Unicode);
    let screen = h.screen();
    for text in [
        "Search apps and packages",
        "All",
        "Audio & video",
        "Fonts",
        "Sources",
        "Popular apps",
        "See all",
        "Firefox",
        "Web browser",
        "Repo · Flatpak",
        "Popular in the AUR",
        "Visual Studio Co",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(recorded.calls().is_empty(), "nothing ran before init");
    assert!(!screen.contains("app catalog"), "nothing is said about the catalog before it is looked for:\n{screen}");
}

#[test]
fn installed_cards_say_so_with_a_mark_and_a_word() {
    let h = started(96, 30);
    assert!(h.screen().contains("Repo · Installed ✓"), "{}", h.screen());
}

#[test]
fn the_network_ranking_replaces_the_starter_list_without_moving_the_rows() {
    let recorded = online();
    let mut unread = page(&recorded, true);
    unread.init = false;
    let mut h = harness(unread, 96, 30, GlyphMode::Unicode);
    let before = h.find("Popular in the AUR").expect("the AUR row stands");
    let first_before = h.app().store.popular[0].key.clone();
    // The same answers the background work gets, delivered one by one.
    let loaded = Arc::new(data::load(&machine(&recorded, true)));
    h.send(Msg::Loaded(loaded));
    h.send(Msg::Pkgstats(data::pkgstats(recorded.as_ref(), None).map(Arc::from)));
    let names = aur_row_names(&h.app().store);
    h.send(Msg::AurStats(data::aur_info(recorded.as_ref(), &names).map(Arc::from)));
    let screen = h.screen();
    assert_eq!(h.find("Popular in the AUR"), Some(before), "the rows stay where they were:\n{screen}");
    let ranked: Vec<&str> = h.app().store.popular.iter().take(3).map(|card| card.key.as_str()).collect();
    assert_eq!(ranked, ["org.mozilla.firefox", "org.videolan.vlc", "libreoffice-writer"], "pkgstats' order");
    assert_ne!(first_before, "", "the starter list had a first card");
    assert!(screen.contains("LibreOffice Writ"), "the catalog's own name:\n{screen}");
    assert!(screen.contains("Google Chrome"), "{screen}");
    assert!(screen.contains("AUR · 2.2k votes"), "the AUR's votes join the card:\n{screen}");
    let chrome = h.find("Google Chrome").expect("Chrome leads the AUR row").0;
    let code = h.find("Visual Studio Co").expect("VS Code follows").0;
    assert!(chrome < code, "the AUR's popularity orders the row:\n{screen}");
}

#[test]
fn every_glyph_mode_draws_an_icon_on_every_card() {
    use qpackages_core::catalog::category::Category;
    for mode in [GlyphMode::Nerd, GlyphMode::Unicode, GlyphMode::Ascii] {
        let recorded = online();
        let h = harness(page(&recorded, true), 96, 30, mode);
        let screen = h.screen();
        let firefox = format!("{} Firefox", icon("firefox", Category::Internet, h.env().icons()));
        assert!(screen.contains(&firefox), "`{firefox}` in {mode:?}:\n{screen}");
        let vlc = format!("{} VLC", icon("vlc", Category::AudioVideo, h.env().icons()));
        assert!(screen.contains(&vlc), "`{vlc}` in {mode:?}:\n{screen}");
        let check = h.env().icons().glyph("check").into_owned();
        assert!(screen.contains(&format!("Installed {check}")), "the mark follows the mode:\n{screen}");
    }
    let ascii = harness(page(&online(), true), 96, 30, GlyphMode::Ascii).screen();
    assert!(ascii.contains("@ Firefox"), "{ascii}");
    assert!(ascii.contains("Installed v"), "{ascii}");
}

#[test]
fn a_card_icon_is_muted_and_takes_the_text_colour_when_selected() {
    let mut h = started(96, 30);
    let (x, y) = h.find("◎ Firefox").expect("the Firefox card");
    let quiet = h.fg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0));
    h.send(Msg::Select(Grid::Row(Section::Popular), 0));
    let lit = h.fg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0));
    assert_ne!(quiet, lit, "the icon brightens on the selected card");
    let name = h.fg(u16::try_from(x + 2).unwrap_or(0), u16::try_from(y).unwrap_or(0));
    assert_eq!(lit, name, "and matches the name's colour");
}

#[test]
fn without_the_app_catalog_one_line_offers_it() {
    let recorded = online();
    let mut h = harness(page(&recorded, false), 96, 30, GlyphMode::Unicode);
    let screen = h.screen();
    assert!(screen.contains("Kinds and descriptions need the app catalog."), "{screen}");
    assert!(screen.contains("Firefox"), "the starter list still fills the page:\n{screen}");
    h.click_text("Install");
    let wanted = Offer { source: Source::Pacman, package: CATALOG_PACKAGE.to_owned() };
    assert_eq!(h.app().requests, [Request::Install(vec![wanted])]);
}

#[test]
fn a_kind_turns_the_rows_to_that_kind() {
    let mut h = started(96, 30);
    h.click_text("Games");
    let screen = h.screen();
    assert!(screen.contains("Games: popular"), "{screen}");
    assert!(screen.contains("Steam"), "{screen}");
    assert!(!screen.contains("Firefox"), "{screen}");
}

#[test]
fn a_source_that_is_not_installed_says_so_and_leads_to_the_settings() {
    let mut h = started(96, 30);
    let screen = h.screen();
    assert!(screen.contains("✓ Repo"), "{screen}");
    assert!(screen.contains("○ Snap"), "{screen}");
    h.click_text("○ Snap");
    h.render();
    assert_eq!(h.app().requests, [Request::OpenSettings(Some(Source::Snap))]);
}

#[test]
fn narrow_screens_trade_the_kinds_column_then_the_summary_then_the_source_labels() {
    let wide = started(96, 30).screen();
    assert!(wide.contains("Audio & video"), "{wide}");
    let seventy = started(70, 30).screen();
    assert!(seventy.contains("Kind"), "the kind becomes one line:\n{seventy}");
    assert!(!seventy.contains("Audio & video"), "the column is gone:\n{seventy}");
    assert!(seventy.contains("Web browser"), "{seventy}");
    let fifty = started(50, 30).screen();
    assert!(!fifty.contains("Web browser"), "no summary under 60 columns:\n{fifty}");
    assert!(fifty.contains("Repo · Flatpak"), "{fifty}");
    let thirty = started(36, 30).screen();
    assert!(thirty.contains("Firefox"), "{thirty}");
    assert!(!thirty.contains("Repo"), "no source labels under 40 columns:\n{thirty}");
    let short = started(96, 20).screen();
    assert!(short.contains("Popular apps"), "{short}");
    assert!(!short.contains("Popular in the AUR"), "one row under 24 lines:\n{short}");
}

#[test]
fn turkish_speaks_turkish_all_the_way_to_the_cards() {
    let mut h = started(96, 30);
    h.set_locale("tr");
    let screen = h.screen();
    for text in
        ["Popüler uygulamalar", "Tümü", "Kaynaklar", "Depo · Flatpak", "Kurulu ✓", "AUR'da popüler", "Tümünü gör"]
    {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
}

#[test]
fn see_all_lists_a_row_whole_and_esc_comes_back() {
    let mut h = started(96, 30);
    h.click_text("See all");
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("Back"), "{screen}");
    assert!(screen.contains("Kate"), "cards past the first row:\n{screen}");
    h.press("esc");
    h.advance(Duration::from_secs(1));
    assert!(h.screen().contains("Sources"), "{}", h.screen());
}

#[test]
fn the_app_page_shows_what_the_repositories_say_and_installs_from_there() {
    let recorded = online();
    recorded.answer("pacman", &["-Si", "--", "obs-studio"], &core_fixture("pacman-si-obs-studio.txt"), 0);
    let mut h = harness(page(&recorded, true), 100, 30, GlyphMode::Unicode);
    h.click_text("Audio & video");
    h.click_text("OBS Studio");
    h.render();
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    for text in [
        "Back",
        "OBS Studio",
        "Live stream and record videos",
        "mixes scenes, sources and",
        "Source",
        "Repo",
        "Install",
        "Version  32.2.2-1",
        "License  GPL-2.0-only",
        "Download  6.7 MiB",
        "On disk  24.7 MiB",
        "Repository  extra",
        "Website  obsproject.com",
        "ffmpeg, jansson, libxinerama, libxkbcommon-x11, mbedtls3 and 14 more",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.click_text("Install");
    let obs = Offer { source: Source::Pacman, package: String::from("obs-studio") };
    assert_eq!(h.app().requests, [Request::Install(vec![obs])]);
    h.press("esc");
    h.advance(Duration::from_secs(1));
    assert!(h.screen().contains("Sources"), "esc goes back:\n{}", h.screen());
    assert!(h.app().store.open.is_none());
}

#[test]
fn another_source_shows_its_own_facts() {
    let recorded = online();
    recorded.answer("pacman", &["-Si", "--", "obs-studio"], &core_fixture("pacman-si-obs-studio.txt"), 0);
    let mut h = harness(page(&recorded, true), 100, 30, GlyphMode::Unicode);
    h.click_text("Audio & video");
    h.click_text("OBS Studio");
    h.send(Msg::Offer(1));
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("Flatpak id  com.obsproject.Studio"), "{screen}");
    assert!(screen.contains("Flatpak"), "{screen}");
    h.click_text("Install");
    let flatpak = Offer { source: Source::Flatpak, package: String::from("com.obsproject.Studio") };
    assert_eq!(h.app().requests, [Request::Install(vec![flatpak])], "the request names the source chosen");
}

#[test]
fn an_installed_package_offers_removal_in_the_danger_tone() {
    let recorded = online();
    recorded.answer("pacman", &["-Si", "--", "firefox"], &core_fixture("pacman-si-obs-studio.txt"), 0);
    let mut h = harness(page(&recorded, true), 100, 30, GlyphMode::Unicode);
    h.click_text("Firefox");
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("Remove"), "{screen}");
    h.click_text("Remove");
    let firefox = Offer { source: Source::Pacman, package: String::from("firefox") };
    assert_eq!(h.app().requests, [Request::Remove(vec![firefox])]);
}

#[test]
fn details_that_cannot_be_read_say_so_and_the_page_still_stands() {
    let recorded = online();
    let mut h = harness(page(&recorded, true), 100, 30, GlyphMode::Unicode);
    h.click_text("VLC");
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("Repo could not be reached for the details."), "{screen}");
    assert!(screen.contains("Install"), "{screen}");
}

#[test]
fn space_checks_cards_and_ctrl_enter_installs_them() {
    let mut h = started(96, 30);
    h.send(Msg::Toggle(Grid::Row(Section::Popular), 0));
    h.send(Msg::Toggle(Grid::Row(Section::Aur), 0));
    assert_eq!(h.app().store.checked.len(), 2);
    h.press("/");
    h.press("ctrl+enter");
    let [Request::Install(offers)] = h.app().requests.as_slice() else {
        panic!("one install request: {:?}", h.app().requests)
    };
    let sources: Vec<Source> = offers.iter().map(|offer| offer.source).collect();
    assert_eq!(sources, [Source::Pacman, Source::Flatpak], "each card's most trusted source");
    h.send(Msg::Toggle(Grid::Row(Section::Popular), 0));
    assert_eq!(h.app().store.checked.len(), 1, "a second toggle unchecks");
}

#[test]
fn learning_the_catalog_is_missing_moves_nothing_on_screen() {
    let recorded = online();
    let mut unread = page(&recorded, false);
    unread.init = false;
    let mut h = harness(unread, 96, 30, GlyphMode::Unicode);
    let before = (h.find("Popular apps"), h.find("Popular in the AUR"));
    h.send(Msg::Loaded(Arc::new(data::load(&machine(&recorded, false)))));
    assert!(h.screen().contains("need the app catalog"), "{}", h.screen());
    assert_eq!((h.find("Popular apps"), h.find("Popular in the AUR")), before, "{}", h.screen());
}

#[test]
fn an_installed_flatpak_is_marked_and_offers_removal() {
    let recorded = online();
    let list = qpackages_core::catalog::flatpak::list_args();
    recorded.answer("flatpak", &list, &core_fixture("flatpak-list-apps.txt"), 0);
    let mut h = harness(page(&recorded, true), 100, 30, GlyphMode::Unicode);
    assert!(recorded.command_lines().contains(&list_line(&list)), "{:?}", recorded.command_lines());
    let screen = h.screen();
    let spotify = screen.lines().skip_while(|line| !line.contains("Spotify")).nth(2).unwrap_or_default();
    assert!(spotify.contains("Flatp… · Installed ✓"), "the source gives way, the mark stays whole:\n{screen}");
    let wide = harness(page(&recorded, true), 130, 30, GlyphMode::Unicode).screen();
    assert!(wide.contains("Flatpak · Installed ✓"), "with room the card names where it is installed from:\n{wide}");
    h.click_text("Spotify");
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("Remove"), "{screen}");
    h.click_text("Remove");
    let spotify = Offer { source: Source::Flatpak, package: String::from("com.spotify.Client") };
    assert_eq!(h.app().requests, [Request::Remove(vec![spotify])]);
}

#[test]
fn without_flatpak_nothing_is_installed_from_it_and_nothing_is_asked_when_it_is_off() {
    // No recording for `flatpak list`: the program cannot be run.
    let h = started(100, 30);
    assert!(!h.app().store.installed.flatpaks.contains_key("com.spotify.client"));
    let recorded = online();
    let asked = |recorded: &Recorded| recorded.command_lines().iter().any(|line| line.starts_with("flatpak"));
    let mut store = Store::new(machine(&recorded, true), vec![Source::Pacman]);
    let read = Some(store.machine_read(&sources(), []));
    let h = harness(Page { store, requests: Vec::new(), held: false, init: true, read }, 100, 30, GlyphMode::Unicode);
    assert!(!asked(&recorded), "Flatpak is off: {:?}", recorded.command_lines());
    assert!(h.screen().contains("Firefox"));
    let mut store = Store::new(machine(&recorded, true), crate::sources::ALL.to_vec());
    let read = Some(store.machine_read(&Sources { flatpak: Availability::Missing, ..sources() }, []));
    let _ = harness(Page { store, requests: Vec::new(), held: false, init: true, read }, 100, 30, GlyphMode::Unicode);
    assert!(!asked(&recorded), "this machine has no Flatpak: {:?}", recorded.command_lines());
}

fn list_line(args: &[String]) -> String {
    std::iter::once("flatpak").chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
}

#[test]
fn flathubs_latest_updates_have_a_row_of_their_own_when_flatpak_is_on() {
    let mut h = started(96, 30);
    let screen = h.screen();
    let aur = h.find("Popular in the AUR").expect("the AUR row");
    let recent = h.find("Recently updated").expect("the row of updates");
    assert!(aur.1 < recent.1, "it comes after the AUR row:\n{screen}");
    for text in ["Halftone", "Dither your images", "FFaudioConverter", "Flatpak"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    h.click_text("Audio & video");
    assert!(h.screen().contains("Audio & video: recently updated"), "{}", h.screen());
    assert!(!h.screen().contains("Halftone"), "a graphics app is not audio:\n{}", h.screen());
    h.set_locale("tr");
    assert!(h.screen().contains("Ses ve video: yeni güncellenenler"), "{}", h.screen());

    let recorded = online();
    let mut store = Store::new(machine(&recorded, true), vec![Source::Pacman, Source::Aur]);
    let read = Some(store.machine_read(&sources(), []));
    let h = harness(Page { store, requests: Vec::new(), held: false, init: true, read }, 96, 30, GlyphMode::Unicode);
    assert!(!h.screen().contains("Recently updated"), "not without Flatpak:\n{}", h.screen());
    let asked = recorded.command_lines().into_iter().any(|line| line.contains("recently-updated"));
    assert!(!asked, "nor asked for");
}

#[test]
fn a_latest_update_opens_its_page_and_installs_as_a_flatpak() {
    let mut h = started(96, 30);
    h.click_text("Halftone");
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("Flatpak id  io.github.tfuxu.Halftone"), "{screen}");
    h.click_text("Install");
    let halftone = Offer { source: Source::Flatpak, package: String::from("io.github.tfuxu.Halftone") };
    assert_eq!(h.app().requests, [Request::Install(vec![halftone])]);
}

#[test]
fn every_short_kind_name_fits_beside_a_three_digit_count_in_both_languages() {
    let mut h = started(96, 30);
    for language in ["en", "tr"] {
        h.set_locale(language);
        for kind in Kind::column(true) {
            let short = h.env().i18n().translate(&kind.short_key(), &[]);
            let width = qframe::text::width(&short) + view::COUNTED_OVERHEAD + 3;
            assert!(width <= view::KINDS_WIDTH, "`{short}` in {language} takes {width} cells with a count");
        }
    }
}

/// The widths an application's page is held to. Under 70 columns the store drops the summary and
/// the source labels from its cards, and the page's two columns of facts become one.
const NARROW: [u16; 3] = [70, 100, 120];

/// The page of the application `key` on the popular row, with the repositories' answer for every
/// package a test may open recorded. The card is opened by message, so no width and no language
/// has to be aimed at.
fn app_page(key: &str, width: u16) -> Harness<Page> {
    let recorded = online();
    for package in ["firefox", "vlc"] {
        recorded.answer("pacman", &["-Si", "--", package], &core_fixture("pacman-si-obs-studio.txt"), 0);
    }
    let mut h = harness(page(&recorded, true), width, 40, GlyphMode::Unicode);
    let index = h
        .app()
        .store
        .popular
        .iter()
        .position(|card| card.key == key)
        .unwrap_or_else(|| panic!("`{key}` is on the popular row"));
    h.send(Msg::Open(Grid::Row(Section::Popular), index));
    h.advance(Duration::from_secs(1));
    h
}

#[test]
fn an_application_page_shows_its_fixed_labels_whole_in_every_language() {
    // A label the page cannot fit is shortened with an ellipsis rather than left out, so a page
    // that draws something is no proof. Each label is read out of the language file and looked
    // for as it is written: a shortened "Herunterladen" is no longer that string.
    let shared = [
        "store.back",
        "store.app.source",
        "store.app.version",
        "store.app.license",
        "store.app.download",
        "store.app.on-disk",
        "store.app.repository",
        "store.app.website",
        "store.app.depends",
    ];
    // VLC is not on this pretend machine and Firefox is, so between them the page shows both of
    // the buttons it can offer.
    let applications = [("org.videolan.vlc", "store.app.install"), ("org.mozilla.firefox", "store.app.remove")];
    for code in crate::locales::tests::codes() {
        for width in NARROW {
            for (key, button) in applications {
                let mut h = app_page(key, width);
                h.set_locale(&code);
                let screen = h.screen();
                for label in shared.iter().chain([&button]) {
                    let text = crate::locales::tests::label(&code, label);
                    assert!(
                        screen.contains(&text),
                        "`{label}` is cut off in `{code}` on {key} at {width} columns; it reads `{text}`:\n{screen}"
                    );
                }
            }
        }
    }
}

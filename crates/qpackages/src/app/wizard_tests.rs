//! The first start, driven from what the person touches: the wizard opens while qpac has no
//! `packages.conf`, writes nothing until Finish, and then hands the checked sources to the normal
//! confirmation and the background check to the same timer the Settings page switches.
//!
//! Every test lives in a scratch machine: the family folder, the fonts the appearance step looks
//! at, the unit folder the timer is written into and every program qpac runs are its own, so
//! neither the user's settings nor a real font, timer or package is ever touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::runtime::ProcessOutcome;
use qpackages_core::helper::PACMAN_PATH;
use qpackages_core::pacman::command::{PACMAN, install, print_install};
use qpackages_core::sources::Source;

use crate::app::{Qpackages, Tab};
use crate::autostart::{self, SYSTEMCTL, TIMER};
use crate::locales::tests::label;
use crate::runner::Recorded;
use crate::testing::{Sample, Scratch, click_last, first_start, programs};

/// What `pacman -S --print` says for flatpak: one package, so the dialog's title is fixed.
const FLATPAK_PLAN: &str = "extra|flatpak|1.16.1-1|2000000\n";

/// A recording that answers the timer's systemctl calls and flatpak's plan, and nothing else:
/// flatpak's installation itself is not recorded, so it could not run even if it were asked.
fn recording() -> Arc<Recorded> {
    let recorded = Recorded::default();
    recorded.answer(SYSTEMCTL, &autostart::reload_args(), "", 0);
    recorded.answer(SYSTEMCTL, &autostart::enable_args(), "", 0);
    recorded.answer(SYSTEMCTL, &autostart::disable_args(), "", 0);
    recorded.answer(PACMAN, &print_install(&["flatpak"]), FLATPAK_PLAN, 0);
    Arc::new(recorded)
}

/// A scratch machine with pacman, paru and fakeroot, neither flatpak nor snap.
fn machine(name: &str) -> Scratch {
    Scratch::new(name, &[Sample::new("bash", "5.3-1", "Shell")])
}

/// Lets what was started run: the first read, a save, a plan.
fn settle(h: &mut Harness<Qpackages>) {
    for _ in 0..6 {
        h.advance(Duration::from_millis(20));
    }
}

/// qpac's first start on `scratch` in English, with the first read answered.
fn start(scratch: &Scratch, recorded: &Arc<Recorded>, width: u16, height: u16) -> Harness<Qpackages> {
    let app = first_start(scratch, recorded, programs, 1000);
    let mut h = Harness::with_env(app, crate::locales::env(), width, height);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    settle(&mut h);
    h
}

/// What the family folder holds, by name, in order; empty when there is no folder at all.
fn folder(scratch: &Scratch) -> Vec<String> {
    let Ok(entries) = fs::read_dir(scratch.config()) else { return Vec::new() };
    let mut names: Vec<String> =
        entries.map(|entry| entry.expect("entry").file_name().to_string_lossy().into()).collect();
    names.sort();
    names
}

/// qpac's own settings file as the wizard left it.
fn written(scratch: &Scratch) -> String {
    fs::read_to_string(scratch.config().join("packages.conf")).unwrap_or_default()
}

/// Presses the wizard's button `label`: the buttons stand at the bottom, under anything else
/// that might carry the same word.
fn press(h: &mut Harness<Qpackages>, label: &str) {
    click_last(h, label);
    settle(h);
}

/// Answers No on the background step: a click on the row's name gives the list the keys, and
/// Space switches the row's switch, as it would for anyone using the keyboard.
fn turn_down_the_background_check(h: &mut Harness<Qpackages>) {
    h.click_text("Check in the background");
    h.press("space");
    settle(h);
    assert!(!h.app().choices.background, "the switch was turned off:\n{}", h.screen());
}

/// The colour the first letter of `text` is drawn in.
fn colour_of(h: &Harness<Qpackages>, text: &str) -> Option<qframe::color::Rgb> {
    let (x, y) = h.find(text).unwrap_or_else(|| panic!("`{text}` is not on screen"));
    h.fg(u16::try_from(x).expect("a column"), u16::try_from(y).expect("a row"))
}

/// The systemctl calls made, as command lines.
fn systemctl(recorded: &Recorded) -> Vec<String> {
    recorded.command_lines().into_iter().filter(|line| line.starts_with(SYSTEMCTL)).collect()
}

#[test]
fn it_opens_while_qpac_has_no_settings_file_and_not_once_it_has_one() {
    let scratch = machine("wizard-opens");
    let recorded = recording();
    let h = start(&scratch, &recorded, 100, 30);
    assert!(h.app().setting_up(), "the first start asks:\n{}", h.screen());
    let screen = h.screen();
    for text in ["qpac", "Appearance", "Sources", "Updates", "In every Quvyta application", "Start with the defaults"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Discover"), "no page of qpac's own is drawn under it:\n{screen}");
    assert!(h.is_focused("setup-appearance"), "the keys start on the appearance rows");
    assert_eq!(folder(&scratch), Vec::<String>::new(), "nothing is written before it is asked");

    // Someone who has used qpac before keeps their file and is never asked.
    fs::create_dir_all(scratch.config()).expect("folder");
    fs::write(scratch.config().join("packages.conf"), "[aur]\nhelper = \"yay\"\n").expect("settings");
    let again = start(&scratch, &recorded, 100, 30);
    assert!(!again.app().setting_up(), "{}", again.screen());
    assert!(again.screen().contains("Discover"), "the normal screen opens at once:\n{}", again.screen());
}

#[test]
fn closing_it_half_way_leaves_the_folder_as_it_was() {
    let scratch = machine("wizard-half-way");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 100, 30);
    press(&mut h, "Next");
    h.click_text("Flatpak");
    press(&mut h, "Next");
    turn_down_the_background_check(&mut h);
    assert_eq!(folder(&scratch), Vec::<String>::new(), "half-way through, the folder is not even made");
    assert!(!scratch.units().exists(), "no timer is written before Finish");
    assert_eq!(systemctl(&recorded), Vec::<String>::new(), "systemctl is not asked before Finish");
    drop(h);

    // Next start: the wizard is there again, with nothing remembered.
    let mut again = start(&scratch, &recorded, 100, 30);
    assert!(again.app().setting_up(), "{}", again.screen());
    press(&mut again, "Next");
    assert!(!again.app().checked(Source::Flatpak), "nothing was remembered");
}

#[test]
fn the_sources_step_checks_what_this_machine_has_and_offers_the_rest() {
    let scratch = machine("wizard-sources");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    press(&mut h, "Next");
    let screen = h.screen();
    for text in [
        "Where qpac finds packages.",
        "Pacman",
        "The official repositories. Always on",
        "AUR",
        "packages are built through paru.",
        "Flatpak",
        "Checked, flatpak is installed after setup",
        "Snap",
        "Checked, snapd is built from the AUR after setup",
        "a /snap link is made. Each step asks you first.",
    ] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    for forbidden in ['[', ']', '{', '}', '|'] {
        assert!(!screen.contains(forbidden), "`{forbidden}`:\n{screen}");
    }
    let app = h.app();
    assert!(app.checked(Source::Pacman) && app.checked(Source::Aur), "what the machine has is checked");
    assert!(!app.checked(Source::Flatpak) && !app.checked(Source::Snap), "what it lacks is not");

    // pacman stays on whatever is clicked: without it there is nothing to manage. Its name is
    // drawn faint, like any control that cannot be changed, and the AUR's is not.
    assert_ne!(colour_of(&h, "Pacman"), colour_of(&h, "AUR"), "pacman's name is faint:\n{}", h.screen());
    h.click_text("Pacman");
    assert!(h.app().checked(Source::Pacman));

    // Snap is built from the AUR, so without the AUR it cannot be checked, and its line says why.
    h.click_text("Snap");
    assert!(h.app().checked(Source::Snap), "a click checks it");
    h.click_text("AUR");
    assert!(!h.app().checked(Source::Aur));
    assert!(!h.app().checked(Source::Snap), "without the AUR, Snap cannot come");
    assert!(h.screen().contains("so it needs the AUR checked above."), "{}", h.screen());
    h.click_text("Snap");
    assert!(!h.app().checked(Source::Snap), "and a click does not check it");
}

#[test]
fn a_missing_source_checked_is_asked_for_after_finish_and_nothing_runs_unconfirmed() {
    let scratch = machine("wizard-install");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    press(&mut h, "Next");
    h.click_text("Flatpak");
    press(&mut h, "Next");
    press(&mut h, "Finish");
    assert!(!h.app().setting_up(), "the wizard is over:\n{}", h.screen());
    let screen = h.screen();
    assert!(screen.contains("Install 1 package?"), "the normal confirmation asks:\n{screen}");
    assert!(screen.contains("flatpak"), "{screen}");
    assert!(
        !recorded.command_lines().iter().any(|line| line.starts_with(PACMAN_PATH)),
        "nothing is installed before it is confirmed: {:?}",
        recorded.command_lines()
    );
    assert!(h.handoffs().is_empty(), "no permission is asked before it is confirmed");
    // Flatpak is on by default, so the file does not need to say so.
    assert!(!written(&scratch).contains("flatpak"), "{}", written(&scratch));

    // Cancelled, it is still offered where it always is.
    click_last(&mut h, "Cancel");
    settle(&mut h);
    assert!(!h.screen().contains("Install 1 package?"), "{}", h.screen());
    h.click_text("Discover");
    assert!(!recorded.command_lines().iter().any(|line| line.starts_with(PACMAN_PATH)));
}

#[test]
fn two_missing_sources_are_asked_for_one_after_the_other() {
    let scratch = machine("wizard-two");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    press(&mut h, "Next");
    h.click_text("Flatpak");
    h.click_text("Snap");
    press(&mut h, "Next");
    press(&mut h, "Finish");
    assert!(h.screen().contains("Install 1 package?"), "flatpak is asked first:\n{}", h.screen());
    assert!(!h.screen().contains("snapd"), "snapd waits its turn:\n{}", h.screen());
    // Turned down, flatpak gives its turn to snapd, whose build is planned the way the Settings
    // page's button plans it; the recording has no AUR to answer, so the plan says so.
    click_last(&mut h, "Cancel");
    settle(&mut h);
    let screen = h.screen();
    assert!(screen.contains("The build could not be planned"), "snapd's turn came:\n{screen}");
    let lines = recorded.command_lines();
    assert!(lines.iter().any(|line| line.starts_with("curl") && line.contains("snapd")), "{lines:?}");
    // snapd did not come, so neither its service nor its link is asked for.
    assert!(h.app().after_setup.is_empty(), "{:?}", h.app().after_setup);
    assert!(!screen.contains("Turn snapd on?"), "{screen}");
    assert!(!lines.iter().any(|line| line.starts_with(PACMAN_PATH)), "nothing was installed: {lines:?}");
}

/// Whether the pretend machine has paru yet: it has not until a test says its installation ran.
static PARU_THERE: AtomicBool = AtomicBool::new(false);

/// A machine with pacman and fakeroot, and paru once [`PARU_THERE`] says so.
fn paru_once_installed(program: &str) -> Option<PathBuf> {
    let there = ["pacman", "fakeroot"].contains(&program) || (program == "paru" && PARU_THERE.load(Ordering::SeqCst));
    there.then(|| Path::new("/usr/bin").join(program))
}

#[test]
fn snapd_waits_until_the_paru_it_is_built_with_is_installed_and_read_again() {
    let scratch = machine("wizard-paru-first");
    let recorded = recording();
    recorded.answer(PACMAN, &print_install(&["paru"]), "extra|paru|2.0.4-1|3000000\n", 0);
    recorded.play(
        PACMAN_PATH,
        &install(&["paru"]),
        &["(1/1) installing paru"],
        ProcessOutcome::Finished { code: Some(0) },
    );
    let app = first_start(&scratch, &recorded, paru_once_installed, 1000);
    let mut h = Harness::with_env(app, crate::locales::env(), 120, 40);
    h.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    settle(&mut h);
    press(&mut h, "Next");
    assert!(h.screen().contains("Checked, paru is installed after setup"), "{}", h.screen());
    h.click_text("AUR");
    h.click_text("Snap");
    press(&mut h, "Next");
    press(&mut h, "Finish");
    assert!(h.screen().contains("Install 1 package?") && h.screen().contains("paru"), "{}", h.screen());
    // The installation runs from the recording; from then on the machine has paru.
    PARU_THERE.store(true, Ordering::SeqCst);
    click_last(&mut h, "Install");
    settle(&mut h);
    settle(&mut h);
    let screen = h.screen();
    assert!(
        recorded.command_lines().contains(&format!("{PACMAN_PATH} -S --needed --noconfirm -- paru")),
        "{:?}",
        recorded.command_lines()
    );
    // Started before paru was read again, the build would have found nothing to build with.
    assert!(!screen.contains("Building from the AUR needs paru or yay"), "{screen}");
    assert!(screen.contains("The build could not be planned"), "snapd's build was planned with paru:\n{screen}");
}

#[test]
fn an_unchecked_source_is_written_off_and_is_off_on_the_main_screen() {
    let scratch = machine("wizard-off");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    assert!(h.screen().contains("Appearance"));
    press(&mut h, "Next");
    h.click_text("AUR");
    press(&mut h, "Next");
    press(&mut h, "Finish");
    let text = written(&scratch);
    assert!(text.contains("aur = false"), "{text}");
    assert_eq!(h.app().tab(), Tab::Discover);
    let screen = h.screen();
    assert!(!screen.contains("Popular in the AUR"), "Discover leaves the AUR out:\n{screen}");
    assert!(screen.contains("Popular apps"), "{screen}");
}

#[test]
fn yes_turns_the_timer_on_with_the_interval_chosen() {
    let scratch = machine("wizard-timer");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    press(&mut h, "Next");
    press(&mut h, "Next");
    assert!(h.screen().contains("installs anything by itself."), "{}", h.screen());
    h.click_text("6 hours");
    h.advance(Duration::from_millis(300));
    h.click_text("12 hours");
    settle(&mut h);
    press(&mut h, "Finish");
    let timer = fs::read_to_string(scratch.units().join(TIMER)).expect("the timer is written");
    assert!(timer.contains("OnUnitActiveSec=12h"), "{timer}");
    let text = written(&scratch);
    assert!(text.contains("autostart = \"on\"") && text.contains("interval = 12"), "{text}");
    assert!(
        systemctl(&recorded).contains(&"systemctl --user enable --now -- quvyta-packages-check.timer".to_owned()),
        "{:?}",
        systemctl(&recorded)
    );
}

#[test]
fn no_leaves_the_timer_alone() {
    let scratch = machine("wizard-no-timer");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    press(&mut h, "Next");
    press(&mut h, "Next");
    turn_down_the_background_check(&mut h);
    press(&mut h, "Finish");
    assert!(!h.app().setting_up());
    assert!(!scratch.units().join(TIMER).exists(), "no timer is written");
    assert_eq!(systemctl(&recorded), Vec::<String>::new(), "systemctl is never asked");
    assert!(!written(&scratch).contains("autostart"), "off is the default:\n{}", written(&scratch));
}

#[test]
fn starting_with_the_defaults_keeps_the_sources_found_and_checks_in_the_background() {
    let scratch = machine("wizard-defaults");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 40);
    h.click_text("Start with the defaults");
    settle(&mut h);
    assert!(!h.app().setting_up(), "{}", h.screen());
    assert_eq!(folder(&scratch), ["packages.conf", "quvyta.conf"]);
    let text = written(&scratch);
    // What the machine has stays on, which is the default and not written; what it lacks and
    // nobody checked is off, so Discover does not offer it.
    assert!(text.contains("flatpak = false") && text.contains("snap = false") && !text.contains("aur"), "{text}");
    assert!(text.contains("autostart = \"on\"") && !text.contains("interval"), "{text}");
    let timer = fs::read_to_string(scratch.units().join(TIMER)).expect("the timer is written");
    assert!(timer.contains("OnUnitActiveSec=6h"), "{timer}");
    assert!(!h.screen().contains("Install 1 package?"), "nothing is asked to be installed");

    // Next start: the file is there, so nothing is asked.
    let again = start(&scratch, &recorded, 120, 40);
    assert!(!again.app().setting_up(), "{}", again.screen());
}

#[test]
fn the_look_chosen_in_the_wizard_is_the_one_the_settings_page_carries_on_from() {
    let scratch = machine("wizard-look");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 50);
    h.click_text("English");
    h.click_text("Türkçe");
    settle(&mut h);
    assert!(h.screen().contains("Kaynaklar"), "the wizard turns Turkish at once:\n{}", h.screen());
    h.click_text("Monochrome");
    h.advance(Duration::from_millis(300));
    h.click_text("Nordic");
    settle(&mut h);
    press(&mut h, "İleri");
    assert!(h.screen().contains("qpac paketleri nerede bulsun?"), "{}", h.screen());
    press(&mut h, "İleri");
    press(&mut h, "Bitir");
    let shared = fs::read_to_string(scratch.config().join("quvyta.conf")).expect("written");
    assert!(shared.contains("language = \"tr\""), "the family keeps it:\n{shared}");
    h.click_text("Keşfet");
    h.press("ctrl+,");
    settle(&mut h);
    let screen = h.screen();
    assert!(screen.contains("Ayarlar") && screen.contains("Türkçe"), "{screen}");
    // The Settings page's theme row starts from the theme chosen, not from the one detected.
    let row = screen.lines().find(|line| line.contains("Renk teması")).expect("the theme row");
    assert!(row.contains("Nordic"), "{screen}");
}

/// Clicks the `nth` place, counting from 0, where `text` starts on screen.
fn click_nth(h: &mut Harness<Qpackages>, text: &str, nth: usize) {
    let screen = h.screen();
    let (y, line) = screen
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(text))
        .nth(nth)
        .unwrap_or_else(|| panic!("`{text}` is not on screen {} times:\n{screen}", nth + 1));
    let x = qframe::text::width(&line[..line.find(text).expect("found")]);
    h.click(i32::from(x), i32::try_from(y).expect("a row"));
}

#[test]
fn a_theme_kept_to_qpac_in_the_wizard_stays_qpac_s_own_on_the_settings_page() {
    let scratch = machine("wizard-own-theme");
    let recorded = recording();
    let mut h = start(&scratch, &recorded, 120, 60);
    h.click_text("Monochrome");
    h.advance(Duration::from_millis(300));
    h.click_text("Nordic");
    settle(&mut h);
    // The theme's "In every Quvyta application" row, the second of the three: a click on it
    // gives it the keys and Space unchecks its box.
    click_nth(&mut h, "In every Quvyta application", 1);
    h.press("space");
    settle(&mut h);
    press(&mut h, "Next");
    press(&mut h, "Next");
    press(&mut h, "Finish");
    assert!(written(&scratch).contains("theme = \"nordic\""), "{}", written(&scratch));
    h.press("ctrl+,");
    settle(&mut h);
    h.click_text("Nordic");
    h.advance(Duration::from_millis(300));
    h.click_text("Amber");
    settle(&mut h);
    // The Settings page knows the theme is qpac's own, as the wizard left it: the new one goes
    // into qpac's file, and the family's file is not touched.
    assert!(written(&scratch).contains("theme = \"amber\""), "{}", written(&scratch));
    let shared = fs::read_to_string(scratch.config().join("quvyta.conf")).expect("written");
    assert!(!shared.contains("amber"), "{shared}");
}

#[test]
fn the_wizard_s_labels_stand_whole_and_its_buttons_inside_the_frame() {
    let steps: [&[&str]; 2] = [
        &["wizard.step-sources", "source.pacman", "source.aur", "source.flatpak", "source.snap"],
        &["wizard.step-updates", "settings-page.check", "settings-page.interval"],
    ];
    for code in crate::locales::tests::codes() {
        // The buttons are the framework's, so their words come from its own catalogue.
        let mut words = qframe::i18n::I18n::builtin();
        assert!(words.set_active(&code), "the framework does not speak `{code}`");
        let buttons = ["back", "next", "finish"].map(|key| words.translate(&format!("quvyta.wizard.{key}"), &[]));
        let buttons = buttons.each_ref().map(String::as_str);
        let code = code.as_str();
        for width in [36, 60, 80, 120] {
            let scratch = machine("wizard-labels");
            let recorded = recording();
            let mut h = start(&scratch, &recorded, width, 40);
            h.set_locale(code);
            for (step, keys) in steps.iter().enumerate() {
                press(&mut h, buttons[1]);
                let screen = h.screen();
                for key in *keys {
                    let text = label(code, key);
                    assert!(screen.contains(&text), "`{key}` is cut in `{code}` at {width}, step {step}:\n{screen}");
                }
                let wanted = if step == 1 { [buttons[0], buttons[2]] } else { [buttons[0], buttons[1]] };
                for button in wanted {
                    assert!(screen.contains(button), "`{button}` in `{code}` at {width}, step {step}:\n{screen}");
                }
                for line in screen.lines() {
                    assert!(qframe::text::width(line) <= width, "`{line}` is too wide in `{code}` at {width}");
                }
                assert!(!screen.contains('⟦'), "a key is missing in `{code}`:\n{screen}");
            }
        }
    }
}

#[test]
fn no_real_settings_folder_is_ever_given_to_a_test() {
    let scratch = machine("wizard-own-folder");
    let recorded = recording();
    let app = first_start(&scratch, &recorded, programs, 1000);
    assert_eq!(app.setup_folder.as_deref(), Some(scratch.config().as_path()));
}

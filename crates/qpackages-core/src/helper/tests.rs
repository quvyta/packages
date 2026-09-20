use super::*;
use crate::backup::Snapshot;
use crate::reflector::Mirrors;

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn install_and_remove_accept_one_or_more_valid_names() {
    assert_eq!(Request::parse("install firefox"), Ok(Request::Install(owned(&["firefox"]))));
    assert_eq!(
        Request::parse("install gtk3 lib32-glibc python-pip libc++ gnome@2 a.b_c"),
        Ok(Request::Install(owned(&["gtk3", "lib32-glibc", "python-pip", "libc++", "gnome@2", "a.b_c"])))
    );
    assert_eq!(Request::parse("remove yay bash"), Ok(Request::Remove(owned(&["yay", "bash"]))));
}

#[test]
fn size_takes_two_positive_numbers() {
    assert_eq!(Request::parse("size 80 24"), Ok(Request::Size { cols: 80, rows: 24 }));
    assert_eq!(Request::parse("size 65535 1"), Ok(Request::Size { cols: 65535, rows: 1 }));
}

#[test]
fn an_unknown_word_is_refused() {
    for line in [
        "",
        "Upgrade",
        "upgrade-all",
        "INSTALL firefox",
        "install_built /tmp/x.pkg.tar.zst",
        " install firefox",
        "sh -c id",
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::Unknown), "`{line}`");
    }
}

#[test]
fn missing_or_extra_values_are_refused() {
    for line in ["install", "remove", "size", "size 80", "size 80 24 1", "size 80 -1", "size 0 24", "size 80 65536"] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
    for line in ["size +80 24", "size 8a 24", "size  80 24", "size 80 24 "] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
}

#[test]
fn names_breaking_the_rule_are_refused() {
    let longest = "a".repeat(255);
    assert_eq!(Request::parse(&format!("install {longest}")), Ok(Request::Install(vec![longest.clone()])));
    let too_long = "a".repeat(256);
    for line in [
        "install ",
        "install firefox ",
        "install  firefox",
        "install -firefox",
        "install --overwrite=*",
        "install .hidden",
        "install Firefox",
        "install fire/fox",
        "install ../etc",
        "install fire\tfox",
        "install firefox\r",
        "install fırefox",
        "remove bash --cascade",
        &format!("install {too_long}"),
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::Name), "`{line}`");
    }
}

#[test]
fn the_new_requests_take_names_as_install_does() {
    for (word, request) in [
        ("upgrade-install", Request::UpgradeInstall as fn(Vec<String>) -> Request),
        ("remove-orphans", Request::RemoveOrphans),
        ("mark-deps", Request::MarkDeps),
        ("mark-explicit", Request::MarkExplicit),
    ] {
        assert_eq!(Request::parse(&format!("{word} gtk3 libc++")), Ok(request(owned(&["gtk3", "libc++"]))), "{word}");
        assert_eq!(Request::parse(word), Err(Refusal::Values), "{word} without names");
        for bad in ["-gtk3", "--asexplicit", ".x", "Gtk3", "a/b", &"a".repeat(256)] {
            assert_eq!(Request::parse(&format!("{word} {bad}")), Err(Refusal::Name), "{word} {bad}");
        }
        assert_eq!(Request::parse(&format!("{word} gtk3 ")), Err(Refusal::Name), "{word} with a trailing space");
    }
}

#[test]
fn upgrade_takes_no_value() {
    assert_eq!(Request::parse("upgrade"), Ok(Request::Upgrade));
    for line in ["upgrade ", "upgrade firefox", "upgrade --overwrite=*"] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
}

#[test]
fn timer_switches_only_reflectors_timer() {
    assert_eq!(Request::parse("timer on reflector.timer"), Ok(Request::Timer(true)));
    assert_eq!(Request::parse("timer off reflector.timer"), Ok(Request::Timer(false)));
    for line in [
        "timer",
        "timer on",
        "timer on sshd.service",
        "timer ON reflector.timer",
        "timer on reflector.timer extra",
        "timer on ../reflector.timer",
        "timer toggle reflector.timer",
        "timer on reflector.service",
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
}

#[test]
fn snapshot_names_its_tool_and_only_snappers_after_takes_a_number() {
    assert_eq!(Request::parse("snapshot pre snapper"), Ok(Request::Snapshot(Snapshot::SnapperPre)));
    assert_eq!(Request::parse("snapshot post snapper 42"), Ok(Request::Snapshot(Snapshot::SnapperPost(42))));
    assert_eq!(Request::parse("snapshot pre timeshift"), Ok(Request::Snapshot(Snapshot::Timeshift)));
    for line in [
        "snapshot",
        "snapshot pre",
        "snapshot pre snapper 42",
        "snapshot post snapper",
        "snapshot post snapper 0",
        "snapshot post snapper 042",
        "snapshot post snapper +42",
        "snapshot post snapper -1",
        "snapshot post snapper 4294967296",
        "snapshot post snapper 4 2",
        "snapshot post snapper ٤٢",
        "snapshot post timeshift 42",
        "snapshot pre btrfs",
        "snapshot pre snapper --description=x",
        "snapshot during snapper",
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
    assert_eq!(
        Request::Snapshot(Snapshot::SnapperPost(7)).command().map(|(program, _)| program),
        Some("/usr/bin/snapper")
    );
}

#[test]
fn mirrors_takes_only_checked_values() {
    let expected = Mirrors::from_values(&["https", "12", "10", "rate", "TR"]).expect("valid");
    assert_eq!(Request::parse("mirrors https 12 10 rate TR"), Ok(Request::Mirrors(expected)));
    assert_eq!(Request::parse("mirrors https 12 10 rate"), Ok(Request::Mirrors(Mirrors::default())));
    for line in [
        "mirrors",
        "mirrors https 12 10",
        "mirrors https 12 10 rate TR DE",
        "mirrors https 12 10 rate United States",
        "mirrors https 12 10 rate --save=/etc/shadow",
        "mirrors https 0 10 rate",
        "mirrors https 12 1000 rate",
        "mirrors https 12 10 rate  TR",
        "mirrors file 12 10 rate",
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
    assert_eq!(Request::Mirrors(Mirrors::default()).command(), None, "the helper handles the files around it");
}

#[test]
fn flatpak_system_removes_one_or_more_valid_ids() {
    assert_eq!(
        Request::parse("flatpak-system remove org.mozilla.firefox"),
        Ok(Request::FlatpakSystemRemove(owned(&["org.mozilla.firefox"])))
    );
    assert_eq!(
        Request::parse("flatpak-system remove org.gimp.GIMP io.github.celluloid_player.Celluloid org.example.a-b"),
        Ok(Request::FlatpakSystemRemove(owned(&[
            "org.gimp.GIMP",
            "io.github.celluloid_player.Celluloid",
            "org.example.a-b"
        ])))
    );
    let longest = format!("a.b.{}", "c".repeat(251));
    assert_eq!(longest.len(), 255);
    assert_eq!(
        Request::parse(&format!("flatpak-system remove {longest}")),
        Ok(Request::FlatpakSystemRemove(vec![longest]))
    );
}

#[test]
fn flatpak_system_refuses_what_is_not_a_removal_of_ids() {
    // Installing and updating are the user's own and never reach the helper.
    for line in [
        "flatpak-system",
        "flatpak-system remove",
        "flatpak-system install org.mozilla.firefox",
        "flatpak-system update org.mozilla.firefox",
        "flatpak-system uninstall org.mozilla.firefox",
        "flatpak-system REMOVE org.mozilla.firefox",
        "flatpak-system --system remove org.mozilla.firefox",
        "flatpak-system org.mozilla.firefox",
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::Values), "`{line}`");
    }
    for line in
        ["flatpak org.mozilla.firefox", "flatpak-user remove org.mozilla.firefox", "Flatpak-system remove a.b.c"]
    {
        assert_eq!(Request::parse(line), Err(Refusal::Unknown), "`{line}`");
    }
}

#[test]
fn flatpak_ids_breaking_the_rule_are_refused() {
    let too_long = format!("a.b.{}", "c".repeat(252));
    for id in [
        "",
        "firefox",
        "org.firefox",
        "-org.mozilla.firefox",
        "--system",
        "--delete-data",
        "org..mozilla",
        "../../etc.passwd.x",
        ".org.mozilla.firefox",
        "org.mozilla.firefox.",
        "org.mozilla.fire/fox",
        "org.mozilla.fırefox",
        "org.mozilla.fire\tfox",
        "org.mozilla.firefox\r",
        "app/org.mozilla.firefox/x86_64/stable",
        "org.mozilla.firefox//stable",
        too_long.as_str(),
    ] {
        let line = format!("flatpak-system remove {id}");
        assert_eq!(Request::parse(&line), Err(Refusal::FlatpakId), "`{line}`");
    }
    for line in [
        "flatpak-system remove org.mozilla.firefox ",
        "flatpak-system remove  org.mozilla.firefox",
        "flatpak-system remove org.mozilla.firefox -x.y.z",
        "flatpak-system remove org.mozilla.firefox --system",
    ] {
        assert_eq!(Request::parse(line), Err(Refusal::FlatpakId), "`{line}`");
    }
}

#[test]
fn flatpak_system_runs_flatpak_by_its_path_for_the_system_with_a_double_dash() {
    let request = Request::FlatpakSystemRemove(owned(&["org.gimp.GIMP", "org.videolan.VLC"]));
    assert_eq!(
        request.command(),
        Some((
            "/usr/bin/flatpak",
            owned(&["--system", "uninstall", "--noninteractive", "--", "org.gimp.GIMP", "org.videolan.VLC"])
        ))
    );
    assert_eq!(request.to_string(), "flatpak-system remove org.gimp.GIMP org.videolan.VLC");
}

#[test]
fn a_request_writes_the_line_it_is_read_from() {
    for request in [
        Request::Install(owned(&["firefox", "vlc"])),
        Request::UpgradeInstall(owned(&["firefox"])),
        Request::Upgrade,
        Request::Remove(owned(&["yay"])),
        Request::RemoveOrphans(owned(&["libfoo", "python-wheel"])),
        Request::MarkDeps(owned(&["gtk3"])),
        Request::MarkExplicit(owned(&["gtk3"])),
        Request::Timer(true),
        Request::Timer(false),
        Request::Snapshot(Snapshot::SnapperPre),
        Request::Snapshot(Snapshot::SnapperPost(4_294_967_295)),
        Request::Snapshot(Snapshot::Timeshift),
        Request::Mirrors(Mirrors::default()),
        Request::Mirrors(Mirrors::from_values(&["http", "24", "5", "age", "TR,DE"]).expect("valid")),
        Request::FlatpakSystemRemove(owned(&["org.gimp.GIMP", "org.videolan.VLC"])),
        Request::InstallBuilt(owned(&["/home/a/.cache/paru/clone/x/x-1-1-any.pkg.tar.zst"])),
        Request::Size { cols: 120, rows: 9 },
    ] {
        assert_eq!(Request::parse(&request.to_string()), Ok(request));
    }
}

#[test]
fn requests_run_programs_by_their_path_with_fixed_flags_and_a_double_dash() {
    let pacman = |args: &[&str]| Some(("/usr/bin/pacman", owned(args)));
    let firefox = || owned(&["firefox"]);
    assert_eq!(Request::Install(firefox()).command(), pacman(&["-S", "--needed", "--noconfirm", "--", "firefox"]));
    assert_eq!(
        Request::UpgradeInstall(firefox()).command(),
        pacman(&["-Syu", "--needed", "--noconfirm", "--", "firefox"])
    );
    assert_eq!(Request::Upgrade.command(), pacman(&["-Syu", "--noconfirm"]));
    assert_eq!(Request::Remove(owned(&["yay"])).command(), pacman(&["-Rns", "--noconfirm", "--", "yay"]));
    assert_eq!(Request::MarkDeps(firefox()).command(), pacman(&["-D", "--asdeps", "--", "firefox"]));
    assert_eq!(Request::MarkExplicit(firefox()).command(), pacman(&["-D", "--asexplicit", "--", "firefox"]));
    let systemctl = |args: &[&str]| Some(("/usr/bin/systemctl", owned(args)));
    assert_eq!(Request::Timer(true).command(), systemctl(&["enable", "--now", "--", "reflector.timer"]));
    assert_eq!(Request::Timer(false).command(), systemctl(&["disable", "--now", "--", "reflector.timer"]));
    assert_eq!(Request::RemoveOrphans(firefox()).command(), None, "the helper lists the orphans first");
    let built = Request::InstallBuilt(owned(&["/home/a/.cache/yay/x/x-1-1-any.pkg.tar.zst"]));
    assert_eq!(built.command(), None, "the helper opens the files first");
    assert_eq!(Request::Size { cols: 80, rows: 24 }.command(), None);
}

#[test]
fn responses_are_read_and_written_the_same_way() {
    for (line, response) in [
        ("ready 1", Response::Ready(1)),
        ("line :: Processing package changes...", Response::Line(":: Processing package changes...".to_owned())),
        ("line  (1/2) installing", Response::Line(" (1/2) installing".to_owned())),
        ("line ", Response::Line(String::new())),
        ("done 0", Response::Done(Some(0))),
        ("done 1", Response::Done(Some(1))),
        ("done signal", Response::Done(None)),
        ("refused not-root", Response::Refused(Refusal::NotRoot)),
        ("refused name", Response::Refused(Refusal::Name)),
        ("refused start", Response::Refused(Refusal::Start)),
        ("refused changed", Response::Refused(Refusal::Changed)),
        ("refused no-mirrors", Response::Refused(Refusal::NoMirrors)),
        ("refused file", Response::Refused(Refusal::File)),
        ("refused flatpak-id", Response::Refused(Refusal::FlatpakId)),
        ("refused built", Response::Refused(Refusal::Built)),
        ("refused not-this-build", Response::Refused(Refusal::NotThisBuild)),
    ] {
        assert_eq!(Response::parse(line), Some(response.clone()), "`{line}`");
        assert_eq!(response.to_string(), line);
    }
    assert_eq!(Response::parse("line"), Some(Response::Line(String::new())));
}

#[test]
fn anything_else_is_not_a_response() {
    for line in ["", "ready", "ready one", "done", "done 1.5", "refused", "refused why", "hello 1", "sudo: a password"]
    {
        assert_eq!(Response::parse(line), None, "`{line}`");
    }
}

#[test]
fn an_output_line_never_breaks_into_two_responses() {
    assert_eq!(Response::Line("one\ntwo".to_owned()).to_string(), "line one two");
}

#[test]
fn every_refusal_has_a_key_that_reads_back() {
    for refusal in Refusal::ALL {
        assert_eq!(Refusal::from_key(refusal.key()), Some(refusal));
    }
    assert_eq!(Refusal::from_key("other"), None);
}

#[test]
fn locales_are_plain_names() {
    for locale in ["C", "tr_TR.UTF-8", "en_US.utf8", "de_DE@euro", "C.UTF-8"] {
        assert!(is_locale(locale), "`{locale}`");
    }
    for locale in ["", "-C", "tr_TR UTF-8", "tr_TR;id", "../x", "1C", &"a".repeat(65)] {
        assert!(!is_locale(locale), "`{locale}`");
    }
}

#[test]
fn the_helper_is_started_without_a_password_prompt_and_with_a_checked_locale() {
    let exe = Path::new("/usr/bin/qpac");
    assert_eq!(
        start_args(exe, Some("tr_TR.UTF-8")),
        ["-n", "/usr/bin/qpac", "--privileged-helper", "--lang", "tr_TR.UTF-8"]
    );
    assert_eq!(start_args(exe, None), ["-n", "/usr/bin/qpac", "--privileged-helper"]);
    assert_eq!(start_args(exe, Some("bad locale")), ["-n", "/usr/bin/qpac", "--privileged-helper"]);
}

#[test]
fn the_helper_reads_only_its_locale_option() {
    assert_eq!(parse_options(&[]), Ok("C".to_owned()));
    assert_eq!(parse_options(&owned(&["--lang", "tr_TR.UTF-8"])), Ok("tr_TR.UTF-8".to_owned()));
    for args in [&["--lang"][..], &["--lang", "x y"], &["--lang", "C", "--lang", "C"], &["--other"], &["C"]] {
        assert_eq!(parse_options(&owned(args)), Err(Refusal::Values), "{args:?}");
    }
}

#[test]
fn pacman_gets_a_fixed_path_and_the_given_locale() {
    assert_eq!(
        pacman_env("tr_TR.UTF-8"),
        [("PATH", "/usr/bin:/usr/sbin"), ("LANG", "tr_TR.UTF-8"), ("LC_ALL", "tr_TR.UTF-8")]
    );
    assert_eq!(PACMAN_PATH, "/usr/bin/pacman");
}

const BUILT: &str = "/home/ayse/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst";

#[test]
fn install_built_takes_one_or_more_absolute_package_paths() {
    assert_eq!(Request::parse(&format!("install-built {BUILT}")), Ok(Request::InstallBuilt(owned(&[BUILT]))));
    let yay =
        ["hello", "hello-debug"].map(|name| format!("/home/ayse/.cache/yay/hello/{name}-1.0-1-x86_64.pkg.tar.xz"));
    assert_eq!(Request::parse(&format!("install-built {}", yay.join(" "))), Ok(Request::InstallBuilt(yay.to_vec())));
    assert_eq!(
        Request::InstallBuilt(owned(&[BUILT])).to_string(),
        format!("install-built {BUILT}"),
        "the line a relay passes on is the line it read"
    );
}

#[test]
fn install_built_refuses_paths_that_break_the_file_rule() {
    assert_eq!(Request::parse("install-built"), Err(Refusal::Values));
    let too_long = format!("/home/ayse/.cache/paru/clone/{}.pkg.tar.zst", "a".repeat(4096));
    for path in [
        "",
        "hello-1.0-1-x86_64.pkg.tar.zst",
        "./hello-1.0-1-x86_64.pkg.tar.zst",
        "~/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst",
        "/home/ayse/.cache/paru/clone/../../../../etc/shadow.pkg.tar.zst",
        "/home/ayse/.cache/paru/clone/./hello/hello-1.0-1-x86_64.pkg.tar.zst",
        "/home/ayse/.cache/paru/clone//hello/hello-1.0-1-x86_64.pkg.tar.zst",
        "/home/ayse/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.gz",
        "/home/ayse/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar",
        "/home/ayse/.cache/paru/clone/hello/PKGBUILD",
        "/home/ayse/.cache/paru/clone/hello/hello.pkg.tar.zst.sig",
        "/home/ayse/.cache/paru/clone/hello/",
        "/.pkg.tar.zst",
        "/home/ayse/.cache/paru/clone/hello/x\u{7}.pkg.tar.zst",
        "--overwrite=*",
        too_long.as_str(),
    ] {
        assert_eq!(Request::parse(&format!("install-built {path}")), Err(Refusal::Built), "`{path}`");
        assert!(!is_built_path(path), "`{path}`");
    }
    assert_eq!(Request::parse(&format!("install-built {BUILT} ../x.pkg.tar.zst")), Err(Refusal::Built), "all or none");
}

#[test]
fn a_built_file_lies_below_one_of_the_users_build_caches() {
    let home = Path::new("/home/ayse");
    for path in [BUILT, "/home/ayse/.cache/yay/hello/hello-1.0-1-any.pkg.tar.zst"] {
        assert!(in_build_cache(Path::new(path), home), "`{path}`");
    }
    for path in [
        "/home/ayse/.cache/paru/clone",
        "/home/ayse/.cache/paru/hello-1.0-1-any.pkg.tar.zst",
        "/home/ayse/.cache/yayx/hello-1.0-1-any.pkg.tar.zst",
        "/home/ayse/hello-1.0-1-any.pkg.tar.zst",
        "/home/mehmet/.cache/paru/clone/hello/hello-1.0-1-any.pkg.tar.zst",
        "/home/ayse/.cache/paru/clone/../../../../etc/x.pkg.tar.zst",
        "/var/cache/pacman/pkg/hello-1.0-1-any.pkg.tar.zst",
    ] {
        assert!(!in_build_cache(Path::new(path), home), "`{path}`");
    }
}

#[test]
fn the_caller_is_read_from_pkexec_then_sudo() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    };
    assert_eq!(caller_uid(env(&[("PKEXEC_UID", "1000"), ("SUDO_UID", "1001")])), Some(1000));
    assert_eq!(caller_uid(env(&[("SUDO_UID", "1001")])), Some(1001));
    assert_eq!(caller_uid(env(&[])), None);
    for bad in ["", "-1", "1000 ", "10a", "99999999999"] {
        let lookup = |name: &str| (name == "PKEXEC_UID").then(|| bad.to_owned());
        assert_eq!(caller_uid(lookup), None, "`{bad}`");
    }
}

#[test]
fn the_callers_home_comes_from_the_password_database() {
    let passwd = "root:x:0:0::/root:/usr/bin/bash\n\
        broken line\n\
        relative:x:1002:1002::home/relative:/bin/sh\n\
        ayse:x:1000:1000:Ayşe:/home/ayse:/usr/bin/zsh\n\
        again:x:1000:1000::/home/again:/bin/sh\n";
    assert_eq!(passwd_home(passwd, 1000), Some(PathBuf::from("/home/ayse")), "the first line of the id");
    assert_eq!(passwd_home(passwd, 0), Some(PathBuf::from("/root")));
    assert_eq!(passwd_home(passwd, 1002), None, "a relative home is no home");
    assert_eq!(passwd_home(passwd, 4242), None);
}

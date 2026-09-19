use super::*;

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
    for line in ["", "upgrade", "INSTALL firefox", "install-built /tmp/x.pkg.tar.zst", " install firefox", "sh -c id"] {
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
fn a_request_writes_the_line_it_is_read_from() {
    for request in [
        Request::Install(owned(&["firefox", "vlc"])),
        Request::Remove(owned(&["yay"])),
        Request::Size { cols: 120, rows: 9 },
    ] {
        assert_eq!(Request::parse(&request.to_string()), Ok(request));
    }
}

#[test]
fn requests_that_run_pacman_carry_the_fixed_flags_and_a_double_dash() {
    assert_eq!(
        Request::Install(owned(&["firefox"])).pacman_args(),
        Some(owned(&["-S", "--needed", "--noconfirm", "--", "firefox"]))
    );
    assert_eq!(Request::Remove(owned(&["yay"])).pacman_args(), Some(owned(&["-Rns", "--noconfirm", "--", "yay"])));
    assert_eq!(Request::Size { cols: 80, rows: 24 }.pacman_args(), None);
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

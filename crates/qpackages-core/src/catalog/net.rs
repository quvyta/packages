//! How the catalog reaches the network: through `curl`, run by the application.
//!
//! `curl` is a dependency of pacman, so it is on every Arch machine, and it takes TLS, proxy and
//! certificate settings from the system. The core only builds the argument list and the URLs;
//! the application runs the program and hands the answer's text to the parsers.

/// The program to run.
pub const CURL: &str = "curl";

/// Seconds a request may take before it counts as failed. A store that waits longer than this on
/// one source is better off showing the others and saying that one did not answer.
pub const TIMEOUT_SECONDS: u32 = 10;

/// The arguments for fetching `url`, program name excluded.
///
/// `--fail` turns an HTTP error into an exit status instead of an error page on stdout, so an
/// answer that reaches a parser was a real answer. `--proto =https` refuses every scheme but
/// HTTPS, including after a redirect. `--globoff` keeps the `[` and `]` a URL may carry from
/// being read as a range, and `--url` keeps a URL from ever being read as an option.
#[must_use]
pub fn curl_args(url: &str) -> Vec<String> {
    [
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        &TIMEOUT_SECONDS.to_string(),
        "--proto",
        "=https",
        "--globoff",
        "--url",
        url,
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Percent-encodes `text` for a URL path segment or query value: everything but the unreserved
/// characters of RFC 3986 becomes `%XX` of its UTF-8 bytes, so a space, `&`, `/` or `ç` in a
/// search term cannot change what the URL asks for.
#[must_use]
pub fn percent_encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_are_the_ones_the_design_asks_for() {
        assert_eq!(
            curl_args("https://aur.archlinux.org/rpc/v5/search/obs?by=name-desc"),
            [
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "10",
                "--proto",
                "=https",
                "--globoff",
                "--url",
                "https://aur.archlinux.org/rpc/v5/search/obs?by=name-desc",
            ]
        );
    }

    #[test]
    fn a_url_that_looks_like_an_option_stays_a_url() {
        let args = curl_args("-o/etc/passwd");
        assert_eq!(args[args.len() - 2..], ["--url", "-o/etc/passwd"]);
    }

    #[test]
    fn only_the_unreserved_characters_survive_encoding() {
        assert_eq!(percent_encode("obs-studio_1.0~x"), "obs-studio_1.0~x");
        assert_eq!(percent_encode("a b&c/d?e=f"), "a%20b%26c%2Fd%3Fe%3Df");
        assert_eq!(percent_encode("çiğ"), "%C3%A7i%C4%9F");
        assert_eq!(percent_encode("c++"), "c%2B%2B");
        assert_eq!(percent_encode("arg[]"), "arg%5B%5D");
    }
}

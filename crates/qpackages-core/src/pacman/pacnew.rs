//! The `.pacnew` files a transaction left behind.
//!
//! When a package brings a new version of a configuration file the user changed, pacman keeps the
//! user's file and puts the new one beside it as `<file>.pacnew`, saying so in one line:
//! `warning: /etc/pacman.conf installed as /etc/pacman.conf.pacnew`. That line is shown to the
//! user in their language, so it is not read word by word: only the absolute paths ending in
//! `.pacnew` are taken from it, and those read the same in every language.

/// The `.pacnew` files named in `lines`, in the order they first appear, each once.
///
/// A path is a word that starts with `/` once anything glued before it (a colour sequence, a
/// quote) is dropped, and that ends in `.pacnew` once closing quotes and punctuation are
/// dropped.
#[must_use]
pub fn pacnew_files<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for line in lines {
        for word in line.split_whitespace() {
            let Some(start) = word.find('/') else { continue };
            // `.pacnew` ends in a letter, so trimming dots here never eats into it.
            let path = word[start..].trim_end_matches(['\'', '"', ',', ';', ':', ')', '.']);
            let named = path.strip_suffix(".pacnew").is_some_and(|file| !file.ends_with('/'));
            if named && !path.contains("/../") && !files.iter().any(|known| known == path) {
                files.push(path.to_owned());
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_file_is_taken_from_pacmans_warning() {
        let lines = [
            "(2/3) upgrading pacman",
            "warning: /etc/pacman.conf installed as /etc/pacman.conf.pacnew",
            "(3/3) upgrading openssh",
        ];
        assert_eq!(pacnew_files(lines), ["/etc/pacman.conf.pacnew"]);
    }

    #[test]
    fn the_paths_read_the_same_in_another_language_and_behind_colour() {
        let lines = [
            "uyarı: /etc/ssh/sshd_config, /etc/ssh/sshd_config.pacnew olarak kuruldu",
            "\u{1b}[1;33mwarning:\u{1b}[0m /etc/locale.gen installed as '/etc/locale.gen.pacnew'.",
            "\u{1b}[0m/etc/mkinitcpio.conf.pacnew",
        ];
        assert_eq!(
            pacnew_files(lines),
            ["/etc/ssh/sshd_config.pacnew", "/etc/locale.gen.pacnew", "/etc/mkinitcpio.conf.pacnew"]
        );
    }

    #[test]
    fn a_file_named_twice_is_listed_once_and_other_lines_give_nothing() {
        let lines = [
            "warning: /etc/pacman.conf installed as /etc/pacman.conf.pacnew",
            "warning: /etc/pacman.conf installed as /etc/pacman.conf.pacnew",
            "warning: /etc/fstab saved as /etc/fstab.pacsave",
            "the word .pacnew alone is not a path",
            "/.pacnew",
            "/etc/../root/x.pacnew",
            "",
        ];
        assert_eq!(pacnew_files(lines), ["/etc/pacman.conf.pacnew"]);
    }
}

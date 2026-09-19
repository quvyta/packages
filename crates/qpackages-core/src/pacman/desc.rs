//! One installed package, read from pacman's own database record.

/// An installed package, as pacman's local database records it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    /// The package name.
    pub name: String,
    /// The installed version, including the release.
    pub version: String,
    /// The one-line description, in English: package metadata is not translated.
    pub description: Option<String>,
    /// The installed size in bytes, exact. `pacman -Qi` rounds this to a unit and loses it.
    pub size: Option<u64>,
    /// When it was installed, as a Unix timestamp.
    pub installed: Option<i64>,
    /// True when the package was asked for, false when it arrived as a dependency.
    pub explicit: bool,
    /// The licenses it is released under.
    pub licenses: Vec<String>,
    /// What it needs, each entry as pacman writes it, version constraints included.
    pub depends: Vec<String>,
    /// What it can use, each entry as `name: what it adds`.
    pub optional: Vec<String>,
    /// The project's home page.
    pub url: Option<String>,
    /// The pacman groups it belongs to, such as `xorg-fonts`: one of the hints its kind is read
    /// from.
    pub groups: Vec<String>,
}

/// Reads one `desc` file from pacman's local database, usually
/// `/var/lib/pacman/local/<name>-<version>/desc`.
///
/// The format is a `%FIELD%` line followed by its values, one per line, up to a blank line.
/// These files are world-readable and never translated, which is why they are read instead of
/// the output of `pacman -Qi`. Fields this version does not know are ignored, and a value that
/// cannot be read is dropped rather than failing the record. A record without `%NAME%` is not a
/// package and returns `None`.
#[must_use]
pub fn parse_desc(text: &str) -> Option<Package> {
    let mut package = Package { explicit: true, ..Package::default() };
    let mut field: Option<&str> = None;
    for line in text.lines() {
        if line.is_empty() {
            field = None;
        } else if let Some(name) = line.strip_prefix('%').and_then(|rest| rest.strip_suffix('%')) {
            field = Some(name);
        } else if let Some(name) = field {
            package.take(name, line);
        }
    }
    (!package.name.is_empty()).then_some(package)
}

impl Package {
    /// Adds one value line to the field named `field`.
    fn take(&mut self, field: &str, value: &str) {
        match field {
            "NAME" => self.name = value.to_owned(),
            "VERSION" => self.version = value.to_owned(),
            "DESC" => self.description = Some(value.to_owned()),
            "URL" => self.url = Some(value.to_owned()),
            "SIZE" => self.size = value.parse().ok(),
            "INSTALLDATE" => self.installed = value.parse().ok(),
            // The field is there only for a package that arrived as a dependency.
            "REASON" => self.explicit = false,
            "LICENSE" => self.licenses.push(value.to_owned()),
            "DEPENDS" => self.depends.push(value.to_owned()),
            "OPTDEPENDS" => self.optional.push(value.to_owned()),
            "GROUPS" => self.groups.push(value.to_owned()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASH: &str = include_str!("../../tests/fixtures/pacman-localdb-desc-bash.txt");

    #[test]
    fn reads_the_real_record_of_bash() {
        let package = parse_desc(BASH).expect("bash is a package");
        assert_eq!(package.name, "bash");
        assert_eq!(package.version, "5.3.15-1");
        assert_eq!(package.description.as_deref(), Some("The GNU Bourne Again shell"));
        assert_eq!(package.size, Some(10_055_508), "the exact byte count, not a rounded MiB");
        assert_eq!(package.installed, Some(1_781_367_195));
        assert!(!package.explicit, "a %REASON% of 1 means it arrived as a dependency");
        assert_eq!(package.licenses, ["GPL-3.0-or-later"]);
        assert_eq!(package.depends, ["readline", "libreadline.so=8-64", "glibc", "ncurses"]);
        assert_eq!(package.optional, ["bash-completion: for tab completion"]);
        assert_eq!(package.url.as_deref(), Some("https://www.gnu.org/software/bash/bash.html"));
    }

    #[test]
    fn groups_are_read_one_per_line() {
        let text = "%NAME%\nxorg-fonts-misc\n\n%VERSION%\n1.0-1\n\n%GROUPS%\nxorg\nxorg-fonts\n";
        let package = parse_desc(text).expect("a font package");
        assert_eq!(package.groups, ["xorg", "xorg-fonts"]);
        assert!(parse_desc(BASH).expect("bash is a package").groups.is_empty(), "bash is in no group");
    }

    #[test]
    fn a_missing_reason_field_means_the_package_was_asked_for() {
        let package = parse_desc("%NAME%\nfoo\n\n%VERSION%\n1.0-1\n").expect("a name and a version are enough");
        assert!(package.explicit);
        assert_eq!(package.size, None);
    }

    #[test]
    fn a_record_without_a_name_is_not_a_package() {
        assert_eq!(parse_desc("%VERSION%\n1.0-1\n"), None);
        assert_eq!(parse_desc(""), None);
    }

    #[test]
    fn an_unreadable_number_is_dropped_and_the_rest_survives() {
        let text = "%NAME%\nfoo\n\n%VERSION%\n1.0-1\n\n%SIZE%\nnot a number\n\n%URL%\nhttps://example.invalid\n";
        let package = parse_desc(text).expect("the record still names a package");
        assert_eq!(package.size, None, "an unreadable size is dropped rather than panicking");
        assert_eq!(package.url.as_deref(), Some("https://example.invalid"));
    }

    #[test]
    fn a_field_this_version_does_not_know_is_ignored() {
        let text = "%NAME%\nfoo\n\n%VERSION%\n1.0-1\n\n%XDATA%\npkgtype=pkg\n";
        let package = parse_desc(text).expect("unknown fields do not spoil the record");
        assert_eq!(package.name, "foo");
    }
}

//! Choosing pacman's mirrors with reflector.
//!
//! qpac checks every value here, on the user's side, and again in the root helper, which runs
//! reflector with a fixed list of options built from those values and nothing else. reflector
//! writes to a temporary file; only a list with at least one server replaces the real one, and
//! the list it replaces is kept beside it.

use std::fmt;
use std::path::Path;

/// reflector by its absolute path.
pub const REFLECTOR_PATH: &str = "/usr/bin/reflector";

/// pacman's mirror list, from the root of the file system.
pub const MIRRORLIST: &str = "etc/pacman.d/mirrorlist";

/// The name the replaced mirror list is kept under, beside the new one.
pub const BACKUP: &str = "mirrorlist.qpac-yedek";

/// reflector's own configuration, read by its timer, from the root of the file system.
pub const CONFIG: &str = "etc/xdg/reflector/reflector.conf";

/// The most countries one request names; far more than anyone picks.
const MAX_COUNTRIES: usize = 64;

/// A country reflector knows mirrors in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Country {
    /// Its English name, as reflector prints it.
    pub name: String,
    /// Its two-letter code.
    pub code: String,
    /// How many mirrors it has.
    pub mirrors: u32,
}

/// The arguments that list the countries: `--list-countries`. It needs no privileges.
#[must_use]
pub fn list_countries_args() -> Vec<String> {
    vec!["--list-countries".to_owned()]
}

/// Reads `reflector --list-countries`: a heading, a rule, then `name code count` lines whose
/// name may hold spaces. The columns are not aligned with the heading, so each line is read from
/// its end.
#[must_use]
pub fn parse_countries(stdout: &str) -> Vec<Country> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace().rev();
            let mirrors = words.next()?.parse().ok()?;
            let code = words.next().filter(|code| is_country_code(code))?;
            let name = words.rev().collect::<Vec<_>>().join(" ");
            (!name.is_empty()).then(|| Country { name, code: code.to_owned(), mirrors })
        })
        .collect()
}

/// Whether `code` is two capital letters, the only shape a country reaches reflector in.
#[must_use]
pub fn is_country_code(code: &str) -> bool {
    code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())
}

/// Which protocol the mirrors are reached over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    /// Encrypted; the default.
    #[default]
    Https,
    /// Unencrypted. Packages are signed, so it is not unsafe, but it shows what is downloaded.
    Http,
}

impl Protocol {
    /// The word reflector takes.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
        }
    }

    /// The protocol written as `word`.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Https, Self::Http].into_iter().find(|protocol| protocol.key() == word)
    }
}

/// How the mirrors are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Fastest download first; the default.
    #[default]
    Rate,
    /// Most recently synchronized first.
    Age,
    /// Best score in the mirror status first.
    Score,
    /// Smallest reported delay first.
    Delay,
}

impl Sort {
    /// The word reflector takes.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Rate => "rate",
            Self::Age => "age",
            Self::Score => "score",
            Self::Delay => "delay",
        }
    }

    /// The order written as `word`.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Rate, Self::Age, Self::Score, Self::Delay].into_iter().find(|sort| sort.key() == word)
    }
}

/// Why a mirror setting was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorError {
    /// Neither a code nor a name reflector lists; the value as given.
    Country(String),
    /// More countries than one request names.
    TooManyCountries,
    /// The count is not between 1 and 100.
    Count,
    /// The age is not between 1 and 720 hours.
    Age,
}

impl fmt::Display for MirrorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Country(country) => write!(formatter, "reflector lists no country `{country}`"),
            Self::TooManyCountries => write!(formatter, "at most {MAX_COUNTRIES} countries"),
            Self::Count => formatter.write_str("the number of mirrors is from 1 to 100"),
            Self::Age => formatter.write_str("the age is from 1 to 720 hours"),
        }
    }
}

impl std::error::Error for MirrorError {}

/// A checked mirror setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirrors {
    countries: Vec<String>,
    protocol: Protocol,
    age: u16,
    count: u8,
    sort: Sort,
}

impl Default for Mirrors {
    /// Every country, https, synchronized in the last 12 hours, the 10 most recent, fastest
    /// first.
    fn default() -> Self {
        Self { countries: Vec::new(), protocol: Protocol::Https, age: 12, count: 10, sort: Sort::Rate }
    }
}

impl Mirrors {
    /// A setting from the user's choices. Each country may be a code or a name as reflector
    /// lists it in `known` (case does not matter), and is kept as its code; none means every
    /// country.
    ///
    /// # Errors
    ///
    /// Returns the first value that is out of range or unknown.
    pub fn new(
        countries: &[&str],
        known: &[Country],
        protocol: Protocol,
        age: u16,
        count: u8,
        sort: Sort,
    ) -> Result<Self, MirrorError> {
        if countries.len() > MAX_COUNTRIES {
            return Err(MirrorError::TooManyCountries);
        }
        let mut codes = Vec::new();
        for country in countries {
            let found = known.iter().find(|known| {
                known.code.eq_ignore_ascii_case(country) || known.name.eq_ignore_ascii_case(country.trim())
            });
            let code =
                found.map(|known| known.code.clone()).ok_or_else(|| MirrorError::Country((*country).to_owned()))?;
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
        Self::checked(codes, protocol, age, count, sort)
    }

    fn checked(
        countries: Vec<String>,
        protocol: Protocol,
        age: u16,
        count: u8,
        sort: Sort,
    ) -> Result<Self, MirrorError> {
        if !(1..=100).contains(&count) {
            return Err(MirrorError::Count);
        }
        if !(1..=720).contains(&age) {
            return Err(MirrorError::Age);
        }
        Ok(Self { countries, protocol, age, count, sort })
    }

    /// Reads the values of a `mirrors` request: protocol, age, count, order, and optionally the
    /// country codes joined by commas. Every rule of [`Mirrors::new`] holds, and a country must be
    /// written as its code; `None` for anything else.
    #[must_use]
    pub fn from_values(values: &[&str]) -> Option<Self> {
        let (fixed, countries) = match values {
            [protocol, age, count, sort] => ([protocol, age, count, sort], None),
            [protocol, age, count, sort, countries] => ([protocol, age, count, sort], Some(*countries)),
            _ => return None,
        };
        let [protocol, age, count, sort] = fixed;
        let digits =
            |text: &str| !text.is_empty() && !text.starts_with('0') && text.bytes().all(|b| b.is_ascii_digit());
        if !digits(age) || !digits(count) {
            return None;
        }
        let codes: Vec<String> = match countries {
            Some(list) => list.split(',').map(str::to_owned).collect(),
            None => Vec::new(),
        };
        let mut unique = codes.clone();
        unique.sort();
        unique.dedup();
        if codes.len() > MAX_COUNTRIES || unique.len() != codes.len() || !codes.iter().all(|code| is_country_code(code))
        {
            return None;
        }
        Self::checked(codes, Protocol::parse(protocol)?, age.parse().ok()?, count.parse().ok()?, Sort::parse(sort)?)
            .ok()
    }

    /// The country codes; empty for every country.
    #[must_use]
    pub fn countries(&self) -> &[String] {
        &self.countries
    }

    /// How many of the most recently synchronized mirrors are kept.
    #[must_use]
    pub fn count(&self) -> u8 {
        self.count
    }

    /// How recently, in hours, a mirror must have synchronized.
    #[must_use]
    pub fn age(&self) -> u16 {
        self.age
    }

    /// How the mirrors are ordered.
    #[must_use]
    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// The request's values, in the order [`Mirrors::from_values`] reads them.
    #[must_use]
    pub fn values(&self) -> Vec<String> {
        let mut values = vec![
            self.protocol.key().to_owned(),
            self.age.to_string(),
            self.count.to_string(),
            self.sort.key().to_owned(),
        ];
        if !self.countries.is_empty() {
            values.push(self.countries.join(","));
        }
        values
    }

    /// The reflector options that pick the mirrors, without where they are saved.
    fn options(&self) -> Vec<String> {
        let mut options = vec![
            "--protocol".to_owned(),
            self.protocol.key().to_owned(),
            "--age".to_owned(),
            self.age.to_string(),
            "--latest".to_owned(),
            self.count.to_string(),
            "--sort".to_owned(),
            self.sort.key().to_owned(),
        ];
        if !self.countries.is_empty() {
            options.extend(["--country".to_owned(), self.countries.join(",")]);
        }
        options
    }

    /// The arguments that make reflector write the list to `save`.
    #[must_use]
    pub fn args(&self, save: &Path) -> Vec<String> {
        let mut args = vec!["--save".to_owned(), save.to_string_lossy().into_owned()];
        args.extend(self.options());
        args
    }

    /// reflector's configuration for its own timer, the same choices one option per line,
    /// saving to pacman's mirror list.
    #[must_use]
    pub fn config(&self) -> String {
        let mut text =
            String::from("# Written by qpac. Changing the mirror settings in qpac writes this file again.\n");
        text.push_str(&format!("--save /{MIRRORLIST}\n"));
        for pair in self.options().chunks(2) {
            text.push_str(&pair.join(" "));
            text.push('\n');
        }
        text
    }
}

/// Whether a mirror list names at least one server: a `Server = …` line that is not commented
/// out.
#[must_use]
pub fn has_server(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start()
            .strip_prefix("Server")
            .is_some_and(|rest| rest.trim_start().strip_prefix('=').is_some_and(|url| !url.trim().is_empty()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const COUNTRIES: &str = include_str!("../tests/fixtures/reflector-list-countries.txt");

    fn known() -> Vec<Country> {
        parse_countries(COUNTRIES)
    }

    #[test]
    fn reads_the_recorded_country_list() {
        let countries = known();
        assert_eq!(countries.len(), 78);
        assert_eq!(countries[0], Country { name: "Albania".to_owned(), code: "AL".to_owned(), mirrors: 1 });
        let united_states = countries.iter().find(|country| country.code == "US").expect("listed");
        assert_eq!(united_states.name, "United States", "a name with a space");
        assert!(countries.iter().all(|country| country.name != "Country"), "the heading is not a country");
    }

    #[test]
    fn countries_are_taken_by_code_or_name_and_kept_as_codes() {
        let mirrors =
            Mirrors::new(&["TR", "germany", "United States", "tr"], &known(), Protocol::Https, 12, 10, Sort::Rate)
                .expect("valid");
        assert_eq!(mirrors.values(), ["https", "12", "10", "rate", "TR,DE,US"]);
        let unknown = Mirrors::new(&["Atlantis"], &known(), Protocol::Https, 12, 10, Sort::Rate);
        assert_eq!(unknown, Err(MirrorError::Country("Atlantis".to_owned())));
        let many: Vec<&str> = std::iter::repeat_n("TR", 65).collect();
        assert_eq!(
            Mirrors::new(&many, &known(), Protocol::Https, 12, 10, Sort::Rate),
            Err(MirrorError::TooManyCountries)
        );
    }

    #[test]
    fn counts_and_ages_stay_in_their_ranges() {
        let make = |age, count| Mirrors::new(&[], &known(), Protocol::Https, age, count, Sort::Rate);
        assert!(make(1, 1).is_ok());
        assert!(make(720, 100).is_ok());
        assert_eq!(make(0, 10), Err(MirrorError::Age));
        assert_eq!(make(721, 10), Err(MirrorError::Age));
        assert_eq!(make(12, 0), Err(MirrorError::Count));
        assert_eq!(make(12, 101), Err(MirrorError::Count));
    }

    #[test]
    fn the_defaults_are_the_designed_ones() {
        assert_eq!(Mirrors::default().values(), ["https", "12", "10", "rate"]);
    }

    #[test]
    fn request_values_read_back_and_nothing_else_does() {
        for values in [
            &["https", "12", "10", "rate"][..],
            &["http", "720", "100", "delay", "TR,DE"],
            &["https", "1", "1", "score", "US"],
        ] {
            let mirrors = Mirrors::from_values(values).expect("valid");
            assert_eq!(mirrors.values(), values);
        }
        for values in [
            &[][..],
            &["https", "12", "10"],
            &["ftp", "12", "10", "rate"],
            &["https", "012", "10", "rate"],
            &["https", "12", "+10", "rate"],
            &["https", "0", "10", "rate"],
            &["https", "12", "101", "rate"],
            &["https", "99999", "10", "rate"],
            &["https", "12", "10", "country"],
            &["https", "12", "10", "rate", "tr"],
            &["https", "12", "10", "rate", "Turkey"],
            &["https", "12", "10", "rate", "TR,"],
            &["https", "12", "10", "rate", "TR,TR"],
            &["https", "12", "10", "rate", ""],
            &["https", "12", "10", "rate", "--save=/x"],
            &["https", "12", "10", "rate", "TR", "DE"],
        ] {
            assert_eq!(Mirrors::from_values(values), None, "{values:?}");
        }
    }

    #[test]
    fn reflector_gets_a_fixed_list_of_options() {
        let mirrors = Mirrors::from_values(&["https", "12", "10", "rate", "TR,DE"]).expect("valid");
        assert_eq!(
            mirrors.args(Path::new("/etc/pacman.d/.mirrorlist.qpac-new")).join(" "),
            "--save /etc/pacman.d/.mirrorlist.qpac-new --protocol https --age 12 --latest 10 --sort rate --country TR,DE"
        );
        assert_eq!(
            Mirrors::default().args(Path::new("/x")).join(" "),
            "--save /x --protocol https --age 12 --latest 10 --sort rate",
            "no country means every country"
        );
        assert_eq!(list_countries_args(), ["--list-countries"]);
    }

    #[test]
    fn the_timer_configuration_holds_the_same_choices() {
        let mirrors = Mirrors::from_values(&["https", "12", "10", "rate", "TR"]).expect("valid");
        let config = mirrors.config();
        let options: Vec<&str> = config.lines().filter(|line| !line.starts_with('#')).collect();
        assert_eq!(
            options,
            [
                "--save /etc/pacman.d/mirrorlist",
                "--protocol https",
                "--age 12",
                "--latest 10",
                "--sort rate",
                "--country TR"
            ]
        );
    }

    #[test]
    fn a_list_needs_a_server_line() {
        assert!(has_server("## Turkey\nServer = https://mirror.example.org/archlinux/$repo/os/$arch\n"));
        assert!(has_server("  Server=https://x/\n"));
        assert!(!has_server("## no mirrors matched\n#Server = https://x/\n"));
        assert!(!has_server("Server =\nServers = x\n"));
        assert!(!has_server(""));
    }
}

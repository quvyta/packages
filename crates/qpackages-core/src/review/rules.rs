//! The rules that point at the lines of a recipe worth a second look.
//!
//! They are plain and explainable on purpose: each one names a pattern a reader can check by
//! eye. They draw attention; they do not prove anything. A recipe that passes every rule is not
//! thereby safe, and the screen says so.

use super::RecipeFile;

/// A rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// `curl`, `wget` or `git clone` while building: code fetched without a checksum.
    NetworkInBuild,
    /// `| sh`, `| bash`, `eval` or `base64 -d`: running code that was fetched or hidden.
    RunsHiddenCode,
    /// `sudo`, `su` or `pkexec`: a recipe never needs privileges.
    AsksPrivilege,
    /// The network or `systemctl enable` in the install script, which runs as root.
    InstallScript,
    /// A source over plain `http://`.
    PlainHttp,
    /// A downloaded source whose checksum is `SKIP` and that is not a version-control checkout.
    SkippedChecksum,
    /// A source host that was not there when the recipe was last approved.
    NewDomain,
    /// The package changed hands since it was last approved.
    NewMaintainer,
    /// A write to the home folder or `/etc` outside `package()`: the build touches the machine
    /// it runs on.
    WritesOutside,
}

impl Rule {
    /// Every rule, in the order they are listed.
    pub const ALL: [Self; 9] = [
        Self::NetworkInBuild,
        Self::RunsHiddenCode,
        Self::AsksPrivilege,
        Self::InstallScript,
        Self::PlainHttp,
        Self::SkippedChecksum,
        Self::NewDomain,
        Self::NewMaintainer,
        Self::WritesOutside,
    ];

    /// The word that names the rule; its explanation is `review.rule.<key>` in the language
    /// files.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::NetworkInBuild => "network-in-build",
            Self::RunsHiddenCode => "runs-hidden-code",
            Self::AsksPrivilege => "asks-privilege",
            Self::InstallScript => "install-script",
            Self::PlainHttp => "plain-http",
            Self::SkippedChecksum => "skipped-checksum",
            Self::NewDomain => "new-domain",
            Self::NewMaintainer => "new-maintainer",
            Self::WritesOutside => "writes-outside",
        }
    }
}

/// One place a rule points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule.
    pub rule: Rule,
    /// The recipe file, as named in the recipe.
    pub file: String,
    /// The line, counted from 1; `None` for a finding about the package rather than a line (a
    /// new maintainer).
    pub line: Option<usize>,
    /// What the rule saw: the source host for [`Rule::NewDomain`], the new maintainer for
    /// [`Rule::NewMaintainer`], empty otherwise.
    pub detail: String,
}

/// The functions whose body builds the package: code fetched here runs unchecked.
const BUILD_FUNCTIONS: [&str; 4] = ["prepare", "build", "check", "package"];

/// The install script's functions that run as root when the package is installed or upgraded.
const INSTALL_FUNCTIONS: [&str; 2] = ["post_install", "post_upgrade"];

/// The checksum arrays pacman knows, without their architecture suffix.
const SUM_ARRAYS: [&str; 8] =
    ["md5sums", "sha1sums", "sha224sums", "sha256sums", "sha384sums", "sha512sums", "b2sums", "cksums"];

/// The version-control prefixes of a source, whose checksum is always `SKIP`.
const VCS: [&str; 5] = ["git", "svn", "hg", "bzr", "fossil"];

/// Runs the line rules over every file of a recipe: the build file (`PKGBUILD`) and the install
/// scripts (`*.install`). Other files, such as patches, are shown in the difference but not
/// scanned: they are data for the build, not commands.
#[must_use]
pub fn scan(files: &[RecipeFile]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for file in files {
        if file.name == "PKGBUILD" {
            scan_pkgbuild(file, &mut findings);
        } else if file.name.ends_with(".install") {
            scan_install(file, &mut findings);
        }
    }
    findings
}

fn scan_pkgbuild(file: &RecipeFile, findings: &mut Vec<Finding>) {
    let mut found = |rule, line: usize| {
        findings.push(Finding { rule, file: file.name.clone(), line: Some(line), detail: String::new() });
    };
    let owners = function_of_each_line(&file.text);
    for (index, (text, owner)) in file.text.lines().zip(&owners).enumerate() {
        let line = index + 1;
        if is_comment(text) {
            continue;
        }
        let words = words(text);
        let building = owner.as_deref().is_some_and(is_build_function);
        if building && fetches(&words) {
            found(Rule::NetworkInBuild, line);
        }
        if runs_hidden_code(text, &words) {
            found(Rule::RunsHiddenCode, line);
        }
        if words.iter().any(|word| matches!(*word, "sudo" | "su" | "pkexec")) {
            found(Rule::AsksPrivilege, line);
        }
        let packaging = owner.as_deref().is_some_and(|name| name == "package" || name.starts_with("package_"));
        if !packaging && writes_to_the_machine(text, &words) {
            found(Rule::WritesOutside, line);
        }
    }
    let sources = arrays(&file.text, |name| name == "source" || name.starts_with("source_"));
    for value in &sources {
        if url_of(&value.text).is_some_and(|url| url.starts_with("http://")) {
            found(Rule::PlainHttp, value.line);
        }
    }
    let sums = arrays(&file.text, |name| {
        SUM_ARRAYS.iter().any(|sum| name == *sum || name.strip_prefix(sum).is_some_and(|rest| rest.starts_with('_')))
    });
    for (array, values) in group(&sums) {
        let suffix = array.split_once('_').map_or("", |(_, arch)| arch);
        let source_name = if suffix.is_empty() { "source".to_owned() } else { format!("source_{suffix}") };
        let sources: Vec<&Value> = sources.iter().filter(|value| value.array == source_name).collect();
        for (index, value) in values.iter().enumerate() {
            let downloaded =
                sources.get(index).is_some_and(|source| url_of(&source.text).is_some() && !is_vcs(&source.text));
            if value.text == "SKIP" && downloaded {
                found(Rule::SkippedChecksum, value.line);
            }
        }
    }
}

fn scan_install(file: &RecipeFile, findings: &mut Vec<Finding>) {
    let owners = function_of_each_line(&file.text);
    for (index, (text, owner)) in file.text.lines().zip(&owners).enumerate() {
        if is_comment(text) || !owner.as_deref().is_some_and(|name| INSTALL_FUNCTIONS.contains(&name)) {
            continue;
        }
        let words = words(text);
        let enables = words.contains(&"systemctl") && words.contains(&"enable");
        if fetches(&words) || enables {
            findings.push(Finding {
                rule: Rule::InstallScript,
                file: file.name.clone(),
                line: Some(index + 1),
                detail: String::new(),
            });
        }
    }
}

/// Whether a build function is named `name`: one of [`BUILD_FUNCTIONS`], or `package_<name>`
/// of a split package.
fn is_build_function(name: &str) -> bool {
    BUILD_FUNCTIONS.contains(&name) || name.starts_with("package_")
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// The words of a shell line: runs of letters, digits and `_ - . + /`, so `sudo`, `su` and
/// `eval` count only as whole words and never inside `sum` or `evaluate`.
fn words(line: &str) -> Vec<&str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || "_-.+/".contains(c))).filter(|word| !word.is_empty()).collect()
}

/// `curl`, `wget` or `git clone`.
fn fetches(words: &[&str]) -> bool {
    words.iter().any(|word| matches!(*word, "curl" | "wget")) || words.windows(2).any(|pair| pair == ["git", "clone"])
}

/// A pipe into a shell, `eval`, or a base64 decode.
fn runs_hidden_code(line: &str, words: &[&str]) -> bool {
    let piped_to_shell = line.split('|').skip(1).any(|after| {
        let program = after.split_whitespace().next().unwrap_or_default();
        matches!(program.rsplit('/').next().unwrap_or_default(), "sh" | "bash" | "zsh" | "dash")
    });
    let decodes = words.windows(2).any(|pair| pair[0] == "base64" && matches!(pair[1], "-d" | "--decode"));
    piped_to_shell || words.contains(&"eval") || decodes
}

/// A home-folder or `/etc` path together with something that writes: a redirection or a
/// command that creates, changes or removes files.
fn writes_to_the_machine(line: &str, words: &[&str]) -> bool {
    let target = ["$HOME", "${HOME}", "~/", " /etc/", "\"/etc/", "'/etc/"].iter().any(|path| line.contains(path))
        || line.trim_start().starts_with("/etc/");
    let writes = line.contains('>')
        || words.iter().any(|word| {
            matches!(*word, "cp" | "mv" | "install" | "mkdir" | "touch" | "rm" | "ln" | "tee" | "chmod" | "chown")
        })
        || words.windows(2).any(|pair| pair[0] == "sed" && pair[1].starts_with("-i"));
    target && writes
}

/// For every line of `text`, the shell function it is inside, if any.
///
/// A function starts at `name() {` or `function name {`; its end is found by counting braces,
/// which is enough for the shape recipes take and never fails: an unbalanced file only leaves
/// the rest of it inside the last function.
fn function_of_each_line(text: &str) -> Vec<Option<String>> {
    let mut owners = Vec::new();
    let mut current: Option<String> = None;
    let mut depth = 0_i64;
    for line in text.lines() {
        if current.is_none()
            && let Some(name) = function_header(line)
        {
            current = Some(name);
            depth = 0;
        }
        owners.push(current.clone());
        if current.is_some() && !is_comment(line) {
            let opens = i64::try_from(line.matches('{').count()).unwrap_or(0);
            let closes = i64::try_from(line.matches('}').count()).unwrap_or(0);
            let was_open = depth > 0 || opens > 0;
            depth += opens - closes;
            if was_open && depth <= 0 {
                current = None;
            }
        }
    }
    owners
}

/// The function a line starts, for `name() {`, `name ()`, and `function name {`.
fn function_header(line: &str) -> Option<String> {
    let line = line.trim_start();
    let keyword = line.strip_prefix("function ").map(str::trim_start);
    let line = keyword.unwrap_or(line);
    // Split packages name their functions `package_<name>`, and package names carry `-`.
    let end = line.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))?;
    let (name, rest) = line.split_at(end);
    let rest = rest.trim_start();
    // `name {` is a function only after the `function` keyword; without it, it is a command.
    let is_function = rest.starts_with("()") || (keyword.is_some() && rest.starts_with('{'));
    (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && is_function).then(|| name.to_owned())
}

/// One value of a shell array, with the array it belongs to and its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Value {
    pub(super) array: String,
    pub(super) text: String,
    pub(super) line: usize,
}

/// The values of every array assignment `name=(…)` whose name passes `wanted`, across lines,
/// with quotes taken off. A plain `name=value` counts as a one-value array.
pub(super) fn arrays(text: &str, wanted: impl Fn(&str) -> bool) -> Vec<Value> {
    let mut values = Vec::new();
    let mut open: Option<String> = None;
    for (index, line) in text.lines().enumerate() {
        let mut rest = line;
        if open.is_none() {
            if is_comment(line) {
                continue;
            }
            let Some((name, value)) = line.trim_start().split_once('=') else { continue };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') || !wanted(name) {
                continue;
            }
            if let Some(inside) = value.strip_prefix('(') {
                open = Some(name.to_owned());
                rest = inside;
            } else {
                let (words, _) = shell_words(value);
                if let Some(word) = words.into_iter().next() {
                    values.push(Value { array: name.to_owned(), text: word, line: index + 1 });
                }
                continue;
            }
        }
        let Some(array) = open.clone() else { continue };
        let (words, closed) = shell_words(rest);
        values.extend(words.into_iter().map(|text| Value { array: array.clone(), text, line: index + 1 }));
        if closed {
            open = None;
        }
    }
    values
}

/// The words of one line inside an array, quotes removed, until a `)` or a `#` outside quotes;
/// and whether the `)` was reached.
fn shell_words(line: &str) -> (Vec<String>, bool) {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, ')' | '#') => {
                if started {
                    words.push(std::mem::take(&mut word));
                }
                return (words, c == ')');
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    (words, false)
}

/// Groups `values` by array, keeping the order the arrays first appear in.
fn group(values: &[Value]) -> Vec<(String, Vec<&Value>)> {
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for value in values {
        match groups.iter_mut().find(|(array, _)| *array == value.array) {
            Some((_, members)) => members.push(value),
            None => groups.push((value.array.clone(), vec![value])),
        }
    }
    groups
}

/// The address a source downloads from, without its `name::` and `vcs+` prefixes and its
/// `#fragment`; `None` for a local file of the recipe.
pub(super) fn url_of(source: &str) -> Option<&str> {
    let source = source.split_once("::").map_or(source, |(_, url)| url);
    let source = source.split_once('#').map_or(source, |(url, _)| url);
    let source =
        VCS.iter().find_map(|vcs| source.strip_prefix(vcs).and_then(|rest| rest.strip_prefix('+'))).unwrap_or(source);
    source.contains("://").then_some(source)
}

/// The host an address points at, lower-cased and without a user or a port.
pub(super) fn host_of(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = host.split(':').next().unwrap_or_default();
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether a source is a version-control checkout, whose checksum cannot be known ahead.
fn is_vcs(source: &str) -> bool {
    let source = source.split_once("::").map_or(source, |(_, url)| url);
    VCS.iter().any(|vcs| source.strip_prefix(vcs).is_some_and(|rest| rest.starts_with('+') || rest.starts_with("://")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkgbuild(text: &str) -> Vec<RecipeFile> {
        vec![RecipeFile { name: "PKGBUILD".to_owned(), text: text.to_owned() }]
    }

    fn rules(files: &[RecipeFile]) -> Vec<(Rule, Option<usize>)> {
        scan(files).into_iter().map(|finding| (finding.rule, finding.line)).collect()
    }

    /// A tidy recipe that no rule points at.
    const CLEAN: &str = "\
# Maintainer: Someone <someone at example dot org>
pkgname=hello
pkgver=2.12
pkgrel=1
arch=('x86_64')
source=(\"https://ftp.gnu.org/gnu/hello/hello-$pkgver.tar.gz\"
        'hello.patch')
sha256sums=('cf04af86dc085268c5f4470fbae49b18afbc221b78096aab842d934a76bad0ab'
            'SKIP')

build() {
  cd \"hello-$pkgver\"
  ./configure --prefix=/usr --sysconfdir=/etc
  make
}

package() {
  cd \"hello-$pkgver\"
  make DESTDIR=\"$pkgdir\" install
  install -Dm644 hello.conf \"$pkgdir/etc/hello.conf\"
}
";

    #[test]
    fn a_tidy_recipe_has_no_findings() {
        assert_eq!(rules(&pkgbuild(CLEAN)), [], "a local file may skip its checksum: it is part of the recipe");
    }

    #[test]
    fn fetching_while_building_is_found_and_fetching_elsewhere_is_not() {
        let text = "pkgver() {\n  curl -s https://x/version\n}\nbuild() {\n  wget https://x/a\n  git clone https://x/b\n}\n\
            package_hello-docs() {\n  curl -o x https://x/c\n}\n";
        assert_eq!(
            rules(&pkgbuild(text)),
            [(Rule::NetworkInBuild, Some(5)), (Rule::NetworkInBuild, Some(6)), (Rule::NetworkInBuild, Some(9))],
            "pkgver() only reads a version"
        );
        assert_eq!(rules(&pkgbuild("build() {\n  git log\n  echo curling\n}\n")), []);
    }

    #[test]
    fn hidden_code_is_found() {
        for line in [
            "curl -s x | sh",
            "cat a |bash",
            "echo x | /bin/sh -s",
            "eval \"$x\"",
            "base64 -d blob",
            "base64 --decode b",
        ] {
            let found = rules(&pkgbuild(&format!("prepare() {{\n{line}\n}}\n")));
            assert!(found.contains(&(Rule::RunsHiddenCode, Some(2))), "`{line}`: {found:?}");
        }
        for line in ["ls | sort", "shasum a", "evaluate x", "base64 file", "echo sh | tee x"] {
            let found = rules(&pkgbuild(&format!("prepare() {{\n{line}\n}}\n")));
            assert!(!found.iter().any(|(rule, _)| *rule == Rule::RunsHiddenCode), "`{line}`: {found:?}");
        }
    }

    #[test]
    fn asking_for_privileges_is_found_as_a_whole_word() {
        for line in ["sudo make install", "su -c 'make'", "pkexec true"] {
            assert_eq!(
                rules(&pkgbuild(&format!("build() {{\n{line}\n}}\n"))),
                [(Rule::AsksPrivilege, Some(2))],
                "`{line}`"
            );
        }
        for line in ["make sum", "cd subdir", "sudoku=1", "# sudo is never needed"] {
            assert_eq!(rules(&pkgbuild(&format!("build() {{\n{line}\n}}\n"))), [], "`{line}`");
        }
    }

    #[test]
    fn the_install_script_is_found_when_it_reaches_out_as_root() {
        let script = "post_install() {\n  systemctl enable --now foo.service\n  curl https://x/ping\n}\n\
            post_upgrade() {\n  wget https://x\n}\npre_remove() {\n  systemctl disable foo\n  curl x\n}\n";
        let files = vec![RecipeFile { name: "hello.install".to_owned(), text: script.to_owned() }];
        assert_eq!(
            rules(&files),
            [(Rule::InstallScript, Some(2)), (Rule::InstallScript, Some(3)), (Rule::InstallScript, Some(6))]
        );
        let quiet = "post_install() {\n  echo 'Run systemctl --user daemon-reload'\n}\n";
        assert_eq!(rules(&[RecipeFile { name: "hello.install".to_owned(), text: quiet.to_owned() }]), []);
    }

    #[test]
    fn a_plain_http_source_is_found() {
        let text =
            "source=(\"http://example.org/a.tar.gz\"\n  'b::git+http://example.org/b'\n  'https://example.org/c')\n";
        assert_eq!(rules(&pkgbuild(text)), [(Rule::PlainHttp, Some(1)), (Rule::PlainHttp, Some(2))]);
        assert_eq!(rules(&pkgbuild("source=(https://example.org/http://x)\n")), []);
    }

    #[test]
    fn a_skipped_checksum_on_a_download_is_found_and_on_a_checkout_is_not() {
        let text = "source=('https://x/a.tar.gz' 'git+https://x/b.git' 'local.patch' 'svn+https://x/c')\n\
            sha256sums=('SKIP' 'SKIP' 'SKIP' 'SKIP')\n\
            source_x86_64=('https://x/bin.tar.gz')\nb2sums_x86_64=('SKIP')\n";
        assert_eq!(rules(&pkgbuild(text)), [(Rule::SkippedChecksum, Some(2)), (Rule::SkippedChecksum, Some(4))]);
        let checked = "source=('https://x/a.tar.gz')\nsha256sums=('0123')\n";
        assert_eq!(rules(&pkgbuild(checked)), []);
    }

    #[test]
    fn writing_to_the_machine_outside_package_is_found() {
        let text = "build() {\n  mkdir -p \"$HOME/.cache/x\"\n  echo x > /etc/hosts\n  cp a ~/b\n  cat $HOME/.config/x\n}\n\
            package() {\n  install -Dm644 a \"$pkgdir/etc/a\"\n  mkdir -p $HOME/x\n}\n";
        assert_eq!(
            rules(&pkgbuild(text)),
            [(Rule::WritesOutside, Some(2)), (Rule::WritesOutside, Some(3)), (Rule::WritesOutside, Some(4))],
            "reading the home folder is not writing, and package() is exempt"
        );
    }

    #[test]
    fn functions_are_told_apart_however_they_are_written() {
        let text = "build ()\n{\n  a\n}\nfunction package {\n  if x; then { b; }; fi\n  c\n}\nd\n";
        let owners = function_of_each_line(text);
        let names: Vec<Option<&str>> = owners.iter().map(Option::as_deref).collect();
        assert_eq!(
            names,
            [
                Some("build"),
                Some("build"),
                Some("build"),
                Some("build"),
                Some("package"),
                Some("package"),
                Some("package"),
                Some("package"),
                None
            ]
        );
    }

    #[test]
    fn arrays_read_across_lines_with_quotes_and_comments() {
        let text = "source=(\"a b\" 'c'\n  d # a comment\n  e)\nsha256sums=x\n";
        let values = arrays(text, |name| name == "source" || name == "sha256sums");
        let read: Vec<(&str, &str, usize)> =
            values.iter().map(|value| (value.array.as_str(), value.text.as_str(), value.line)).collect();
        assert_eq!(
            read,
            [("source", "a b", 1), ("source", "c", 1), ("source", "d", 2), ("source", "e", 3), ("sha256sums", "x", 4)]
        );
    }

    #[test]
    fn addresses_and_hosts_are_read_from_sources() {
        assert_eq!(url_of("name::git+https://github.com/a/b.git#tag=v1"), Some("https://github.com/a/b.git"));
        assert_eq!(url_of("local.patch"), None);
        assert_eq!(host_of("https://user@GitHub.com:443/a"), Some("github.com".to_owned()));
        assert_eq!(host_of("https://$pkgname.org/x"), Some("$pkgname.org".to_owned()));
        assert_eq!(host_of("file:///x"), None);
    }

    #[test]
    fn every_rule_has_its_own_key() {
        let mut keys: Vec<&str> = Rule::ALL.iter().map(|rule| rule.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Rule::ALL.len());
    }
}

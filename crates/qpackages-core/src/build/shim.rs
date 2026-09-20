//! What `qpac --elevate-shim <folder> …` accepts: the pacman calls paru and yay make while they
//! build, each turned into one helper request.
//!
//! paru and yay call their sudo program as `<program> <sudoflags…> pacman <arguments…>`. Their
//! arguments differ in order, in long or short names and in how often `--noconfirm` repeats, so
//! they are parsed as options rather than matched against fixed lists. What is accepted is small
//! and closed, the calls both were seen making:
//!
//! - anywhere: `--noconfirm`, `-q`/`--quiet`, `--config /etc/pacman.conf` and `--`, after which
//!   everything is a value;
//! - `-U`/`--upgrade` with package files: [`Request::InstallBuilt`];
//! - `-D`/`--database` with exactly one of `--asdeps` and `--asexplicit`: [`Request::MarkDeps`]
//!   or [`Request::MarkExplicit`];
//! - `-S`/`--sync` with names, a repository before a `/` dropped: [`Request::Install`];
//! - `-R`/`--remove` with any of `-s`, `-n`, `-u`: [`Request::Remove`];
//! - `-v` alone, sudo's "keep the ticket warm": nothing to do.
//!
//! Everything else is refused, above all a refresh or a system update (`-Sy`, `-Syu`): qpac
//! brings the system up to date itself, and paru and yay are asked for AUR targets only.

use crate::helper::Request;

/// The flag that starts qpac as the shim; the folder of the build's pipes follows it.
pub const FLAG: &str = "--elevate-shim";

/// The program paru and yay name before pacman's arguments.
pub const PACMAN: &str = "pacman";

/// The one configuration file accepted with `--config`: pacman's own, which yay always names.
pub const CONFIG: &str = "/etc/pacman.conf";

/// sudo's flag that only refreshes its ticket.
const VALIDATE: &str = "-v";

/// What one call of the shim asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `sudo -v`: keep the permission warm. The helper is already up, so this succeeds at once.
    Validate,
    /// A pacman call, as the request the helper carries it out with.
    Request(Request),
}

/// pacman's operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Upgrade,
    Database,
    Sync,
    Remove,
}

/// What the options of one pacman call said.
#[derive(Debug, Default)]
struct Options {
    operation: Option<Operation>,
    asdeps: bool,
    asexplicit: bool,
    /// `-s`, `-n` or `-u` was given; only a removal takes them.
    removal: bool,
    values: Vec<String>,
}

impl Options {
    /// Takes `operation`; a second, different one makes the call ambiguous.
    fn operation(&mut self, operation: Operation) -> Option<()> {
        match self.operation.replace(operation) {
            Some(before) if before != operation => None,
            _ => Some(()),
        }
    }
}

/// Whether qpac's whole command line is sudo's bare `-v`: yay's sudo loop, when the user turned it
/// on in yay's own settings, calls its sudo program that way, without the shim's flag and folder.
/// The helper holds the permission already, so qpac answers it at once instead of opening its
/// screen inside yay's terminal.
#[must_use]
pub fn is_bare_validate(args: &[String]) -> bool {
    matches!(args, [only] if only == VALIDATE)
}

/// Reads the arguments that follow the shim's folder. `None` for anything outside the closed set.
#[must_use]
pub fn parse(args: &[String]) -> Option<Call> {
    match args {
        [only] if only == VALIDATE => Some(Call::Validate),
        [program, rest @ ..] if program == PACMAN => pacman(rest).map(Call::Request),
        _ => None,
    }
}

/// The request for pacman called with `args`.
fn pacman(args: &[String]) -> Option<Request> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            options.values.extend(args.by_ref().cloned());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            if name == "config" {
                let path = match inline {
                    Some(path) => path,
                    None => args.next()?,
                };
                (path == CONFIG).then_some(())?;
                continue;
            }
            inline.is_none().then_some(())?;
            long_option(name, &mut options)?;
        } else if let Some(letters) = arg.strip_prefix('-') {
            // A lone `-` would have pacman read its targets from standard input.
            (!letters.is_empty()).then_some(())?;
            for letter in letters.chars() {
                short_option(letter, &mut options)?;
            }
        } else {
            options.values.push(arg.clone());
        }
    }
    request(options)
}

fn long_option(name: &str, options: &mut Options) -> Option<()> {
    match name {
        "noconfirm" | "quiet" => Some(()),
        "upgrade" => options.operation(Operation::Upgrade),
        "database" => options.operation(Operation::Database),
        "sync" => options.operation(Operation::Sync),
        "remove" => options.operation(Operation::Remove),
        "asdeps" => {
            options.asdeps = true;
            Some(())
        }
        "asexplicit" => {
            options.asexplicit = true;
            Some(())
        }
        "recursive" | "nosave" | "unneeded" => {
            options.removal = true;
            Some(())
        }
        _ => None,
    }
}

fn short_option(letter: char, options: &mut Options) -> Option<()> {
    match letter {
        'q' => Some(()),
        'U' => options.operation(Operation::Upgrade),
        'D' => options.operation(Operation::Database),
        'S' => options.operation(Operation::Sync),
        'R' => options.operation(Operation::Remove),
        // With `-R` these are recursive, no-save and unneeded; with `-S` they would search and
        // upgrade the system, which the check in `request` refuses.
        's' | 'n' | 'u' => {
            options.removal = true;
            Some(())
        }
        _ => None,
    }
}

/// The request the options make, when they make exactly one.
fn request(options: Options) -> Option<Request> {
    let Options { operation, asdeps, asexplicit, removal, values } = options;
    let operation = operation?;
    (!values.is_empty()).then_some(())?;
    (!removal || operation == Operation::Remove).then_some(())?;
    ((asdeps || asexplicit) == (operation == Operation::Database)).then_some(())?;
    let request = match operation {
        Operation::Upgrade => Request::InstallBuilt(values),
        Operation::Database if asdeps && !asexplicit => Request::MarkDeps(values),
        Operation::Database if asexplicit && !asdeps => Request::MarkExplicit(values),
        Operation::Database => return None,
        Operation::Sync => {
            Request::Install(values.iter().map(|value| without_repository(value)).collect::<Option<_>>()?)
        }
        Operation::Remove => Request::Remove(values),
    };
    // The request travels as one line and is read again on the other side; only one that reads
    // back as itself is sent, so a value with a space cannot turn into two.
    Request::parse(&request.to_string()).ok().filter(|read| *read == request)
}

/// `extra/scdoc` as `scdoc`: the helper installs by name, and the repository is one pacman finds
/// the name in first anyway.
fn without_repository(value: &str) -> Option<String> {
    match value.split_once('/') {
        None => Some(value.to_owned()),
        Some((repository, name)) => {
            let plain =
                !repository.is_empty() && repository.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
            plain.then(|| name.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> Option<Call> {
        let args: Vec<String> = line.split(' ').map(str::to_owned).collect();
        parse(&args)
    }

    fn request(line: &str) -> Option<String> {
        match call(line)? {
            Call::Request(request) => Some(request.to_string()),
            Call::Validate => Some("validate".to_owned()),
        }
    }

    const BUILT: &str = "/home/builder/.cache/paru/clone/hello/hello-1.0-1-x86_64.pkg.tar.zst";
    const DEBUG: &str = "/home/builder/.cache/yay/hello/hello-debug-1.0-1-x86_64.pkg.tar.zst";

    #[test]
    fn every_call_paru_and_yay_were_seen_making_is_accepted() {
        let cases = [
            // paru: long names, `--noconfirm` twice.
            ("pacman --sync --noconfirm --noconfirm -- extra/scdoc", "install scdoc"),
            ("pacman --database --noconfirm --asdeps -- scdoc", "mark-deps scdoc"),
            (&format!("pacman --upgrade --noconfirm --noconfirm -- {BUILT}"), &format!("install-built {BUILT}")),
            ("pacman --database --noconfirm --asexplicit -- hello", "mark-explicit hello"),
            ("pacman --remove --noconfirm --noconfirm -- scdoc", "remove scdoc"),
            // yay: short names, pacman's configuration named, `-q` anywhere.
            ("pacman -S --noconfirm --config /etc/pacman.conf -- extra/scdoc", "install scdoc"),
            ("pacman -D -q --asdeps --noconfirm --config /etc/pacman.conf -- scdoc", "mark-deps scdoc"),
            ("pacman -D --asexplicit -q --config /etc/pacman.conf -- hello", "mark-explicit hello"),
            (
                &format!("pacman -U --noconfirm --config /etc/pacman.conf -- {BUILT} {DEBUG}"),
                &format!("install-built {BUILT} {DEBUG}"),
            ),
            ("pacman -R -s -u --noconfirm --config /etc/pacman.conf -- scdoc", "remove scdoc"),
        ];
        for (line, wanted) in cases {
            assert_eq!(request(line).as_deref(), Some(wanted), "`{line}`");
        }
    }

    #[test]
    fn options_are_read_whatever_their_spelling_and_order() {
        for (line, wanted) in [
            ("pacman -Rsnu scdoc", "remove scdoc"),
            ("pacman --remove --recursive --nosave --unneeded -- a b", "remove a b"),
            ("pacman --config=/etc/pacman.conf -S core/bash", "install bash"),
            ("pacman --asdeps --database a --quiet", "mark-deps a"),
            ("pacman -S --sync a", "install a"),
            ("pacman -S -- -a", "refused"),
        ] {
            assert_eq!(request(line).as_deref().unwrap_or("refused"), wanted, "`{line}`");
        }
    }

    #[test]
    fn keeping_the_permission_warm_does_nothing() {
        let args = |line: &str| -> Vec<String> { line.split(' ').map(str::to_owned).collect() };
        assert!(is_bare_validate(&args("-v")));
        assert!(!is_bare_validate(&args("-v -v")) && !is_bare_validate(&args("--elevate-shim /x -v")));
        assert!(!is_bare_validate(&[]));
        assert_eq!(call("-v"), Some(Call::Validate));
        assert_eq!(call("-v -v"), None);
        assert_eq!(call("-k"), None);
    }

    #[test]
    fn a_refresh_or_a_system_update_is_refused() {
        for line in [
            "pacman --sync -y -u --noconfirm --",
            "pacman -Sy scdoc",
            "pacman -Syu",
            "pacman -S -y scdoc",
            "pacman -S -u scdoc",
            "pacman -Su scdoc",
            "pacman --sync --refresh scdoc",
            "pacman --sync --sysupgrade scdoc",
            "pacman -Sc",
            "pacman -S --clean",
            "pacman -Ss scdoc",
        ] {
            assert_eq!(call(line), None, "`{line}`");
        }
    }

    #[test]
    fn everything_outside_the_closed_set_is_refused() {
        for line in [
            "",
            "pacman",
            "pacman --noconfirm",
            "pacman -S",
            "pacman -S --",
            "/usr/bin/pacman -S scdoc",
            "sh -c id",
            "pacman -Q scdoc",
            "pacman -T scdoc",
            "pacman -F scdoc",
            "pacman -S -R scdoc",
            "pacman -U -S scdoc",
            "pacman -D scdoc",
            "pacman -D --asdeps --asexplicit scdoc",
            "pacman -S --asdeps scdoc",
            "pacman -U --asdeps /x.pkg.tar.zst",
            "pacman -D --asdeps -s scdoc",
            "pacman -S --overwrite=* scdoc",
            "pacman -S --overwrite * scdoc",
            "pacman -S --needed scdoc",
            "pacman -S --noconfirm=yes scdoc",
            "pacman -S --config /tmp/pacman.conf scdoc",
            "pacman -S --config",
            "pacman -S --root /mnt scdoc",
            "pacman -S --dbpath /tmp scdoc",
            "pacman -S --hookdir /tmp scdoc",
            "pacman -S - scdoc",
            "pacman -S /scdoc",
            "pacman -S ext$ra/scdoc",
            "pacman -S Scdoc",
            "pacman -U relative.pkg.tar.zst",
            "pacman -U /etc/shadow",
            "pacman -U /home/builder/.cache/paru/../x.pkg.tar.zst",
            "pacman -R --cascade scdoc",
            "pacman -Rc scdoc",
            "pacman -Rdd scdoc",
        ] {
            assert_eq!(call(line), None, "`{line}`");
        }
    }

    #[test]
    fn a_value_with_a_space_cannot_become_two() {
        let args: Vec<String> = ["pacman", "-R", "--", "a b"].map(str::to_owned).to_vec();
        assert_eq!(parse(&args), None);
        let args: Vec<String> =
            ["pacman", "-U", "--", "/home/builder/.cache/yay/a b/x.pkg.tar.zst"].map(str::to_owned).to_vec();
        assert_eq!(parse(&args), None);
    }
}

//! Snapshots before a system update, with snapper or timeshift.
//!
//! qpac never installs a snapshot tool itself: setting one up is a decision about the file
//! system (a btrfs layout, its subvolumes) that belongs to the user. It finds what is there,
//! picks a default from it, and knows the exact commands the root helper runs.

use std::fs;
use std::path::Path;

/// snapper by its absolute path.
pub const SNAPPER_PATH: &str = "/usr/bin/snapper";

/// timeshift by its absolute path.
pub const TIMESHIFT_PATH: &str = "/usr/bin/timeshift";

/// The description every snapshot qpac takes carries: a fixed text, so nothing the user or a
/// package wrote ever reaches the command line.
pub const DESCRIPTION: &str = "qpac";

/// A snapshot tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// snapper, with its `root` configuration.
    Snapper,
    /// timeshift.
    Timeshift,
}

impl Tool {
    /// The word that names the tool in the settings file and on the helper's line.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Snapper => "snapper",
            Self::Timeshift => "timeshift",
        }
    }
}

/// The `backup.tool` setting: `off`, `snapper` or `timeshift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// No snapshot is taken.
    Off,
    /// Snapshots are taken with this tool.
    Tool(Tool),
}

impl Setting {
    /// The value as the settings file writes it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Tool(tool) => tool.key(),
        }
    }

    /// The setting written as `value`, when it is one of the three.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "snapper" => Some(Self::Tool(Tool::Snapper)),
            "timeshift" => Some(Self::Tool(Tool::Timeshift)),
            _ => None,
        }
    }

    /// The setting a user who never chose one gets: snapper when it is installed with a `root`
    /// configuration, otherwise timeshift when it is installed, otherwise off.
    #[must_use]
    pub fn default_for(found: &Detected) -> Self {
        if found.snapper && found.snapper_root != Some(false) {
            Self::Tool(Tool::Snapper)
        } else if found.timeshift {
            Self::Tool(Tool::Timeshift)
        } else {
            Self::Off
        }
    }
}

/// What is installed, as far as an ordinary user can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Detected {
    /// Whether snapper is installed.
    pub snapper: bool,
    /// Whether snapper has a `root` configuration; `None` when that cannot be told without
    /// privileges (the configuration folder is not readable), and then nothing is guessed.
    pub snapper_root: Option<bool>,
    /// Whether timeshift is installed.
    pub timeshift: bool,
    /// Whether snap-pac is installed: its pacman hooks take a snapper snapshot around every
    /// transaction already.
    pub snap_pac: bool,
}

/// Looks at the machine whose file system starts at `root`, normally `/`.
///
/// snap-pac is found in pacman's local database, which every user can read, rather than by its
/// hook files, whose names changed between its releases.
#[must_use]
pub fn detect(root: &Path) -> Detected {
    let program = |path: &str| root.join(path.trim_start_matches('/')).is_file();
    let snapper_root = root.join("etc/snapper/configs/root").try_exists().ok();
    let snap_pac = fs::read_dir(root.join("var/lib/pacman/local")).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("snap-pac-"))
                .is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit()))
        })
    });
    Detected { snapper: program(SNAPPER_PATH), snapper_root, timeshift: program(TIMESHIFT_PATH), snap_pac }
}

/// Whether `snapper list-configs` printed a configuration named `root`: the first column of a
/// line other than the heading and its rule.
#[must_use]
pub fn lists_root_config(stdout: &str) -> bool {
    stdout.lines().any(|line| line.split(['|', '│']).next().is_some_and(|name| name.trim() == "root"))
}

/// What happens around an update under a setting on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// No snapshot: the setting is off.
    Off,
    /// snapper was chosen, and snap-pac's hooks take its snapshots already: qpac does not call
    /// snapper too, so there are never two snapshots of one update.
    SnapPac,
    /// The chosen tool is not installed, or snapper has no `root` configuration.
    Unavailable(Tool),
    /// qpac takes the snapshot with this tool.
    Take(Tool),
}

impl Plan {
    /// The plan for `setting` on a machine where `found` is installed.
    #[must_use]
    pub fn new(setting: Setting, found: &Detected) -> Self {
        match setting {
            Setting::Off => Self::Off,
            Setting::Tool(Tool::Snapper) if !found.snapper || found.snapper_root == Some(false) => {
                Self::Unavailable(Tool::Snapper)
            }
            Setting::Tool(Tool::Snapper) if found.snap_pac => Self::SnapPac,
            Setting::Tool(Tool::Timeshift) if !found.timeshift => Self::Unavailable(Tool::Timeshift),
            Setting::Tool(tool) => Self::Take(tool),
        }
    }
}

/// One snapshot the root helper takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snapshot {
    /// snapper's snapshot before the update; it prints its number.
    SnapperPre,
    /// snapper's snapshot after the update, paired with the pre snapshot of this number.
    SnapperPost(u32),
    /// timeshift's snapshot before the update; timeshift has no after.
    Timeshift,
}

impl Snapshot {
    /// The program, by its absolute path, and its fixed arguments.
    #[must_use]
    pub fn command(self) -> (&'static str, Vec<String>) {
        let owned = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        match self {
            Self::SnapperPre => (
                SNAPPER_PATH,
                owned(&[
                    "-c",
                    "root",
                    "create",
                    "--type",
                    "pre",
                    "--cleanup-algorithm",
                    "number",
                    "--print-number",
                    "--description",
                    DESCRIPTION,
                ]),
            ),
            Self::SnapperPost(pre) => {
                let mut args = owned(&["-c", "root", "create", "--type", "post", "--pre-number"]);
                args.push(pre.to_string());
                args.extend(owned(&["--cleanup-algorithm", "number", "--description", DESCRIPTION]));
                (SNAPPER_PATH, args)
            }
            Self::Timeshift => (TIMESHIFT_PATH, owned(&["--create", "--comments", DESCRIPTION, "--scripted"])),
        }
    }
}

/// The number snapper printed for a snapshot with `--print-number`: the last line of `lines`
/// that is a number and nothing else.
#[must_use]
pub fn snapshot_number<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<u32> {
    lines
        .into_iter()
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())).then(|| line.parse().ok()).flatten()
        })
        .last()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A fresh folder standing in for `/`, emptied when a run before was interrupted.
    fn machine(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("qpackages-backup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
            if file.ends_with('/') {
                fs::create_dir_all(&path).expect("a folder");
            } else {
                fs::write(&path, "").expect("a file");
            }
        }
        fs::create_dir_all(&root).expect("the root");
        root
    }

    #[test]
    fn nothing_installed_is_found_as_nothing() {
        let root = machine("bare", &[]);
        let found = detect(&root);
        assert_eq!(found, Detected { snapper_root: Some(false), ..Detected::default() });
        assert_eq!(Setting::default_for(&found), Setting::Off);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapper_with_its_root_configuration_and_snap_pac_are_found() {
        let root = machine(
            "snapper",
            &["usr/bin/snapper", "etc/snapper/configs/root", "var/lib/pacman/local/snap-pac-3.0.1-1/"],
        );
        let found = detect(&root);
        assert_eq!(found, Detected { snapper: true, snapper_root: Some(true), timeshift: false, snap_pac: true });
        assert_eq!(Setting::default_for(&found), Setting::Tool(Tool::Snapper));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_package_whose_name_only_starts_like_snap_pac_is_not_snap_pac() {
        let root = machine("lookalike", &["var/lib/pacman/local/snap-pac-grub-2.0-1/", "usr/bin/timeshift"]);
        let found = detect(&root);
        assert!(!found.snap_pac);
        assert!(found.timeshift);
        assert_eq!(Setting::default_for(&found), Setting::Tool(Tool::Timeshift));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapper_without_a_root_configuration_is_not_the_default() {
        let found = Detected { snapper: true, snapper_root: Some(false), timeshift: true, snap_pac: false };
        assert_eq!(Setting::default_for(&found), Setting::Tool(Tool::Timeshift));
        let unknown = Detected { snapper_root: None, ..found };
        assert_eq!(Setting::default_for(&unknown), Setting::Tool(Tool::Snapper), "an unreadable folder is not a no");
    }

    #[test]
    fn the_configuration_list_names_root() {
        let listed = "Config | Subvolume\n-------+----------\nhome   | /home\nroot   | /\n";
        assert!(lists_root_config(listed));
        assert!(lists_root_config("Config │ Subvolume\n───────┼──────────\nroot   │ /\n"));
        assert!(!lists_root_config("Config | Subvolume\n-------+----------\nhome   | /home\n"));
        assert!(!lists_root_config("rooted | /x\n"));
        assert!(!lists_root_config(""));
    }

    #[test]
    fn the_setting_reads_three_words_and_writes_them_back() {
        for setting in [Setting::Off, Setting::Tool(Tool::Snapper), Setting::Tool(Tool::Timeshift)] {
            assert_eq!(Setting::parse(setting.key()), Some(setting));
        }
        for value in ["", "Off", "btrfs", "snapper ", "auto"] {
            assert_eq!(Setting::parse(value), None, "`{value}`");
        }
    }

    #[test]
    fn the_plan_follows_the_setting_and_what_is_installed() {
        let everything = Detected { snapper: true, snapper_root: Some(true), timeshift: true, snap_pac: false };
        let snapper = Setting::Tool(Tool::Snapper);
        let timeshift = Setting::Tool(Tool::Timeshift);
        assert_eq!(Plan::new(Setting::Off, &everything), Plan::Off);
        assert_eq!(Plan::new(snapper, &everything), Plan::Take(Tool::Snapper));
        assert_eq!(Plan::new(timeshift, &everything), Plan::Take(Tool::Timeshift));
        let with_snap_pac = Detected { snap_pac: true, ..everything };
        assert_eq!(Plan::new(snapper, &with_snap_pac), Plan::SnapPac, "never two snapshots of one update");
        assert_eq!(Plan::new(timeshift, &with_snap_pac), Plan::Take(Tool::Timeshift));
        let no_root = Detected { snapper_root: Some(false), ..with_snap_pac };
        assert_eq!(Plan::new(snapper, &no_root), Plan::Unavailable(Tool::Snapper));
        assert_eq!(Plan::new(snapper, &Detected::default()), Plan::Unavailable(Tool::Snapper));
        assert_eq!(Plan::new(timeshift, &Detected::default()), Plan::Unavailable(Tool::Timeshift));
    }

    #[test]
    fn the_commands_are_the_fixed_ones() {
        let line = |snapshot: Snapshot| {
            let (program, args) = snapshot.command();
            std::iter::once(program.to_owned()).chain(args).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(
            line(Snapshot::SnapperPre),
            "/usr/bin/snapper -c root create --type pre --cleanup-algorithm number --print-number --description qpac"
        );
        assert_eq!(
            line(Snapshot::SnapperPost(42)),
            "/usr/bin/snapper -c root create --type post --pre-number 42 --cleanup-algorithm number --description qpac"
        );
        assert_eq!(line(Snapshot::Timeshift), "/usr/bin/timeshift --create --comments qpac --scripted");
    }

    #[test]
    fn the_printed_number_is_the_last_line_that_is_only_a_number() {
        assert_eq!(snapshot_number(["42"]), Some(42));
        assert_eq!(snapshot_number(["Creating snapshot", " 17 ", ""]), Some(17));
        assert_eq!(snapshot_number(["1", "2"]), Some(2));
        assert_eq!(snapshot_number(["snapshot 42 made", "-3", "4x", "99999999999"]), None);
        assert_eq!(snapshot_number([]), None);
    }
}

# Changelog

Every release of quvyta-packages, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/); while the version starts with 0, a minor release may change the interface, the command line or the files qpac keeps under `~/.config/quvyta/` and `~/.local/state/quvyta/packages/`, and the notes say so when it does.

## 0.1.7 - 2026-09-20

### Added

- A moving picture at the top of the README (also as MP4): Discover, a search answered across the repositories, Flatpak and the AUR, an application's page, the confirmation naming every package pacman will install, the install running with pacman's own output, and the updates waiting with the Arch news above them. It is drawn from the test harness against an invented machine, so nothing on it comes from a real one.
- Snap is a source of its own. snapd is read on its own socket, as you, so listing the snaps and
  following a job's real progress need no privileges, and the helper carries out the install, the
  removal and the update. A snap that needs classic confinement is a separate request, so
  `--classic` can never land on a snap whose confirmation did not say what it means. Snaps appear
  in Discover's search and on an application's page, installed ones are marked on their cards,
  waiting snap updates join the Updates tab as their own group, and the settings show snapd's
  version. Without snapd the source is shown faint with what installing it would take, and
  nothing is asked of a snapd that is not there.

### Changed

- Built on quvyta-framework 0.1.11.

### Fixed

- A failed Snap job said pacman had ended with an error; it now names the program that failed.
- A step that moves no package no longer drops the steps queued after it. This was waiting to
  happen for the mirror list and the background timer, and was first hit by a classic snap, which
  asks for the `/snap` link before it installs.

## 0.1.6 - 2026-09-20

### Added

- Flatpak applications are installed and removed from Discover. One installed for you needs no password; a system-wide one goes through the helper, and a mix of the two is confirmed once and then run step by step. When Flathub is not set up, the settings page says so and offers to add it.
- AUR packages are built and installed: `paru` or `yay` does the building as you, never as root, and the root steps it asks for are relayed to qpac's helper through a private pair of pipes, so the build never gets a shell with privileges. The packages a build wants to install are worked out first and checked against what the helper is then asked to install; a build's own debug packages are marked as dependencies.
- A recipe review before an AUR build: the PKGBUILDs and the files beside them are fetched from the AUR, compared with the ones you approved last time and read through a set of rules that point at the risky lines. The whole build is one screen with the changed lines marked, and only "Reviewed, install" starts it.

### Changed

- The appearance rows of the settings page (language, theme, icons, reduced motion and the pillar) are the family's shared ones: under each shared row a box says whether the change applies in every Quvyta application or here only, and the shared ones are kept in `quvyta.conf` beside the other applications of the family.
- The icons qpac brings of its own (the kinds of software, the sources) are drawn in the icon set the theme chose, in every theme and in every glyph mode, and the package's icon is now part of its name cell rather than part of its text.
- The settings page is one scrolling list again, and the keys keep the chosen row in view instead of jumping to the end of it.
- The settings page is opened by a small icon button in the header, and the Updates tab carries the number waiting as a badge.
- The state and cache folders come from the framework (`~/.local/state/quvyta/packages` and `~/.cache/quvyta/packages`).
- Built on quvyta-framework 0.1.10.

## 0.1.5 - 2026-09-20

### Added

- Update all: the updates are applied in one run, and when Timeshift or Snapper is set up on the machine, qpac offers to take a snapshot first and says which one it used.
- Recent Arch Linux news above the updates, so an update that needs a manual step is read before it runs and not after.
- Orphaned packages: the packages nothing depends on any more are listed in the settings page and can be removed together.
- The mirror list can be refreshed with `reflector`, with the countries chosen on the screen.
- An optional check for updates in the background, off by default; when it is on, the Updates tab carries the number waiting.
- Flatpak applications already installed on the machine appear in the store beside the repository packages.

### Changed

- Built on quvyta-framework 0.1.7.

### Fixed

- The confirmation window with three buttons ("Cancel", "Install only", "Update the system first, then install") is now as wide as its buttons need, so no button is cut off or pushed off the screen in a narrow terminal.

## 0.1.4 - 2026-09-19

### Added

- Discover, a store page: popular applications and popular AUR packages as cards, with the kinds of software beside them. Typing searches the repositories, the AUR and Flatpak at once, results arrive as each source answers without the cards jumping around, and one application found in several sources is one card. Descriptions, categories and icons come from AppStream, the data GNOME Software and KDE Discover use.
- An application's own page: description, version, licence, download and installed size, repository, website and dependencies, with Install or Remove on it.
- Tabs: Discover, Installed and Updates, and a settings page of its own.
- Where polkit is installed, permission is asked through `pkexec` and a small helper of qpac's own that answers requests as root; `privilege.tool` in the settings chooses between that and `sudo`.
- Screenshots at the top of the README.

## 0.1.3 - 2026-09-18

### Added

- Pictures in the README: the list, a package's detail, the confirmation and a run.
- One password per session: the privilege ticket is taken once in the terminal, and the runs after it do not ask again.

### Changed

- Removing a package now takes the libraries only it needed along with it, and the confirmation lists them.

## 0.1.2 - 2026-09-18

### Changed

- The settings file moved into the shared Quvyta folder (`~/.config/quvyta/`), next to the other applications of the family.
- README: a one-line install command, and what to do when the command is not found on the PATH.

## 0.1.1 - 2026-09-18

### Added

- A short command, `qpac`, beside `quvyta-packages`. Both start the same application.

## 0.1.0 - 2026-09-18

The first beta.

### Added

- Installed packages in one list with search, each with its source, version and size, and the selected package's details beside the list.
- Updates from the repositories, with a note beside the packages whose update wants a restart.
- Installing and removing packages from the repositories, with one confirmation that shows everything that will change before anything does.
- pacman runs in a pane on the screen, in a real terminal, so its questions and its output are the ones pacman itself prints.
- qpac itself never runs as root and never sees your password: the steps that need privileges ask for it in the terminal, outside the application.
- In a narrow terminal the screen folds: the list and the details take turns instead of standing side by side.
- Mouse support throughout.
- English and Turkish.

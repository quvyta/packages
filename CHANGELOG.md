# Changelog

Every release of quvyta-packages, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/); while the version starts with 0, a minor release may change the interface, the command line or the files qpac keeps under `~/.config/quvyta/` and `~/.local/state/quvyta/packages/`, and the notes say so when it does.

## 0.1.13 - 2026-09-25

### Added

- qpac follows the Quvyta ecosystem while it is open. When another Quvyta application changes the language, the theme, the icons or reduced motion for every application, or gives qpac a value of its own, qpac takes it at once, and the appearance rows on the settings page say where the next change goes. qpac watches `~/.config/quvyta/` with the system's own file events for this; it reads nothing new and sends nothing.

### Changed

- Reduce motion is shared like the language: its row has the "In every Quvyta application" box, and while it is checked the switch is saved in `~/.config/quvyta/quvyta.conf`.
- Built against quvyta-framework 0.1.29; quvyta-packages-core stays at 0.1.8.

### Fixed

- The README named `Accept-Encoding: gzip` among the headers of the question about a newer qpac; qpac never sends it. The headers are `Host`, `User-Agent` and `Accept: */*`, and nothing else.

## 0.1.12 - 2026-09-24

### Changed

- The recordings the tests replay no longer carry real people's names, user names or email addresses: snap publishers, AUR maintainers and a developer named in the store data are now invented ones, and every address is on `example.com`. Nothing qpac does changed.
- Needs quvyta-packages-core 0.1.8, whose tests changed the same way.
- Built against quvyta-framework 0.1.21.

## 0.1.11 - 2026-09-23

### Changed

- The words of the switch for a newer qpac, the README and the documentation now speak of the Quvyta ecosystem rather than a family of applications. The Turkish texts of that switch and its notice address you as the rest of qpac does.
- Built against quvyta-framework 0.1.19.

## 0.1.10 - 2026-09-23

### Added

- qpac says when a newer qpac is out, as every Quvyta application does. When it starts, at most once a day, it asks crates.io for the newest version of `quvyta-packages`, without waiting for the answer and silently when there is no network; a newer one is shown in a notice that names qpac, says it is about qpac itself and not your packages, and says how to update it. Nothing is asked while the first-run setup is open. It is on by default and switched off under **qpac itself** at the bottom of the settings with **Say when a newer qpac is out**, the family's one switch (`update-notice` in `~/.config/quvyta/quvyta.conf`), which stops the question in every Quvyta application at once. It is a separate thing from the check for updates to your packages, and the settings and the notice say it apart.
- The README lists everything qpac sends over the network and to whom: this question, with its exact address and headers, and what the package checks, Discover, the Arch news, the AUR recipe review, the installations and reflector reach.
- What the background check does with the updates it finds, chosen in the settings and on the first-run setup's updates step: *Tell me* (as before, and still the default), *Download* or *Install*. *Download* fetches the pending updates from the official repositories as you, without root, into `~/.cache/quvyta/packages/downloads`; the next system update you confirm copies them into pacman's own cache, where pacman checks each signature as it does for its own downloads, so it no longer waits for the mirrors. The folder only ever holds what is pending, and going back to *Tell me* empties it. AUR, Flatpak and Snap updates are only reported: building AUR packages ahead would run recipes nobody has reviewed yet, qpac does not update Flatpak applications yet, and snapd refreshes snaps by itself. *Install* runs a root-owned script that takes no arguments, `/usr/lib/quvyta-packages/upgrade`, through a sudoers line the settings show and you add yourself; qpac never writes it. The script comes with a distribution package, so a `cargo install` does not have it: there *Install* is shown with that reason and cannot be chosen.

### Changed

- Needs quvyta-packages-core 0.1.7, which carries the root helper's new step: before a system update it takes the packages downloaded ahead into pacman's cache, only regular files that belong to you, opened without following a link.

## 0.1.9 - 2026-09-23

### Added

- A first-run setup, the same one the other Quvyta applications open with. Its first step is the family's appearance (language, theme, icons). Then qpac's own two: which sources to use, with what this computer has already checked, and whether to check for updates in the background and how often. A source the computer lacks can be checked; once the setup is over it is installed through the normal confirmation, one source after another, and a Snap chosen this way is built from the AUR, then its service is turned on and its `/snap` link made, each asking first. Nothing is written until *Finish*, so a setup closed half-way comes again next time; anyone who already has a `packages.conf` never sees it.

### Changed

- The difference between two versions of an AUR recipe numbers its lines again, and each number is the line's number in its own file: a removed line keeps the old file's number, the others the new file's. The line a finding names now carries the number the finding states.
- Built against quvyta-framework 0.1.18.

## 0.1.8 - 2026-09-20

### Added

- Seven languages beside English and Turkish: German, Spanish, French, Brazilian Portuguese, Russian, Simplified Chinese and Japanese. qpac follows your system language, and finds its file even where the setting names a region or a script (`pt_PT` reads the Brazilian file, `zh_CN` and `zh_SG` the Simplified Chinese one).
- The checks a translation is held to. Every language file must carry every English key and no other, keep every placeholder its English text has, hold exactly the plural forms its own language uses (Russian's four, Chinese's and Japanese's one), and use only characters the embedded fonts can draw, because a character the fonts lack is left out of a picture rather than drawn. Each of those rules is itself shown refusing the mistake it is there for.
- Every page, every dialog and every notice must now show its fixed labels whole in all nine languages at the narrow widths qpac is already held to, and a dialog's buttons must stand inside the panel rather than at its edge. A translation runs longer than English, and a label shortened with an ellipsis is no longer the word the file holds, so the check fails on it.

### Changed

- The mirror list qpac replaces is kept as `mirrorlist.qpac-backup`. It was kept under a Turkish name, which is no name for a file an application in nine languages leaves on disk. A copy an earlier version left as `mirrorlist.qpac-yedek` stays where it is; nothing reads it, and you can delete it.

### Fixed

- The Installed tab's summary line no longer cuts its buttons off on a narrow screen: when *Remove checked* and *Clean up* no longer fit beside the counts, they move to a row of their own, and take one row each when even that is too narrow. It read `R…` at 36 columns, in English as well.
- The pane of a running or failed transaction keeps its control whole: the heading took the whole row and left the button whatever remained, so a German *Close* read `Schli…`. The heading, which carries package names and may honestly be shortened, is the one that gives way now.

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

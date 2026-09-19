# qpac

![qpac: Discover, the store page, with popular apps and popular AUR packages as cards, the kinds of software on the left and the sources below them](https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/discover.png)

**quvyta-packages**, or **qpac** for short, is a package manager for Arch Linux that runs in the
terminal. It is meant to bring pacman, the AUR, Flatpak and Snap together in one simple interface
that feels like an app store, and shows exactly what will change before anything does. It is part
of the Quvyta family of terminal applications, is built on
[quvyta-framework](https://github.com/quvyta/framework) and is open source under the MIT licence.

> **Beta.** qpac is new. It finds software in the repositories, the AUR and Flatpak, and installs
> and removes packages from the repositories; building from the AUR, Flatpak installs and Snap
> come in later releases. The interface may still change between releases. Please report anything
> that looks wrong at <https://github.com/quvyta/packages/issues>.

<p>
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/discover-search.png" alt="A search for obs: one card per application across the repositories, Flatpak and the AUR, with counts per kind on the left" width="49%">
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/discover-app.png" alt="OBS Studio's page: description, version, licence, download and installed sizes, repository, website, dependencies and an Install button" width="49%">
</p>
<p>
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/installed.png" alt="The Installed tab: every package with its source, version and size, neovim selected with its details beside the list, neovim and tmux checked for removal" width="49%">
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/confirm.png" alt="The removal confirmation: neovim and tmux with the libraries only they needed, eight packages in all" width="49%">
</p>
<p>
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/updates.png" alt="The Updates tab: three updates from the repositories with a restart note beside the kernel, a recent Arch news item above them, Check now and Update all" width="49%">
</p>
<p>
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/running.png" alt="The removal running: pacman's output in a pane below the list, the progress at half way and the admin badge in the header" width="49%">
  <img src="https://raw.githubusercontent.com/quvyta/packages/main/docs/screenshots/flatpak.png" alt="The settings page: sources with Flatpak missing and a button to install it, the AUR helper, the background check, snapshots and orphaned packages" width="49%">
</p>

## What it does

- **Discover.** A store page: popular apps and what is popular in the AUR as cards, the kinds of
  software on the left (Internet, Audio & video, Graphics, Office, Games, Development and more).
  Typing searches the repositories, the AUR and Flatpak at once; results arrive as each source
  answers, without the cards jumping around, and one application found in several sources is one
  card. Opening a card shows the application's page with its description, sizes, licence,
  website and dependencies, and an Install or Remove button. Categories and descriptions come
  from AppStream, the data GNOME Software and KDE Discover use; without it (the
  `archlinux-appstream-data` package) qpac offers to install it and works with a built-in list
  meanwhile. Popularity comes from pkgstats, Flathub and the AUR's votes, and is kept for a day so the page opens with it; with Flatpak on, a row shows what Flathub updated recently, and installed Flatpaks are marked.
- **Installed.** Your applications, or every package, in a table you can search and sort, with
  the source of each (`source:aur` in the search keeps only the AUR's), and a detail panel:
  version, installed size, whether you asked for it or it came as a dependency, install date,
  licences, dependencies and website. The list is read straight from pacman's local database, so
  it opens at once and needs no privileges.
- **Updates.** The waiting updates, grouped by source, with a note on those that need a restart,
  and the last two weeks of Arch news above them, those that need your hand marked. The check
  refreshes a private copy of pacman's database and never runs `pacman -Sy` on the real one, so it
  can never leave the system half upgraded. **Update all** shows the full list, takes a snapper or
  timeshift snapshot before (and with snapper after) when one is installed, and lists the new
  `.pacnew` files when it is done. Installing while updates wait offers to update first, so the
  system is never partly upgraded.
- **Orphans.** Packages nothing needs any more are marked among all packages and can be cleaned
  up in one step; after a removal or an update qpac asks, cleans up by itself, or leaves them, as
  you choose.
- **Mirrors.** With reflector installed, the settings choose pacman's mirrors by country, count,
  age and speed, keep the old list beside the new one, and can turn on reflector's weekly timer.
- **Changes you can see.** Install from the store, or check packages and remove them. Before
  anything runs, pacman is asked what the change would do, and the full list, dependencies
  included, is shown for you to confirm. Cancel has the focus, so the safe answer is the default.
- **Your password stays with polkit or sudo.** The first change you confirm asks for it once:
  polkit (`pkexec`) where it is installed, in your desktop's own window or on the terminal, and
  sudo on the terminal otherwise. qpac never sees it. A small helper then carries out every
  change as root until qpac closes, and does nothing else; the `admin` badge in the header shows
  it is up, and clicking it lets the permission go. pacman runs with its output and progress in a
  pane below the list, which stays where it is. A long transaction can be stopped.
- **A locked database is explained, never forced.** If another transaction holds pacman's lock,
  qpac says so, and by which process when it can tell. It never removes the lock.
- **Settings.** The gear at the top right (or `ctrl+,`) opens them: which sources are on, the AUR
  helper, who asks for permission, and the language, theme and icons. A missing source is shown
  with the reason, and where its program is in the official repositories (`paru` for the AUR,
  `flatpak` for Flatpak) it can be installed from there, through the same confirmation. The AUR
  is refused when qpac runs as root, because packages are never built as root.
- **A check in the background, if you want one.** Turned on in the settings, a systemd user
  timer runs `qpac --check` every few hours (6 by default, never more often than hourly): it
  looks for updates without opening the screen and without privileges, and writes what it found
  to `~/.local/state/quvyta-packages/state.json`. Nothing runs as root and nothing is installed.

The interface follows your system language (English and Turkish are included) and uses the
family's themes, icons, keys and mouse behaviour.

### Not yet

- Building from the AUR (through `paru` or `yay`, with a look at the recipe first), Flatpak
  installs and Snap.

## Requirements

- Arch Linux, or a distribution built on it, with `pacman`.
- polkit (`pkexec`) or `sudo`, for anything that changes the system.
- Rust 1.95 or later to build it.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/quvyta/quvyta/main/install.sh | sh -s -- packages
```

Or with cargo:

```sh
cargo install quvyta-packages
qpac
```

If the command is not found, add `~/.cargo/bin` to your `PATH` (fish: `fish_add_path ~/.cargo/bin`).

The program is installed as `qpac` and also under its full name, `quvyta-packages`. Release
0.1.0 called the short command `qpackages`; from 0.1.1 on it is `qpac`, and `cargo install`
removes the old name when it upgrades.

## Using it

Run `qpac` without `sudo`: it asks for privileges only when a change is confirmed.

| Key | What it does |
|---|---|
| `ctrl+1`, `ctrl+2`, `ctrl+3` | Discover, Installed, Updates; `alt+left` and `alt+right` step through them |
| `/` | Search on the page you are on |
| `space` | Check or uncheck the selected package or card |
| `ctrl+enter` | Install the checked cards, after a confirmation |
| `delete` | Remove the checked packages, after a confirmation |
| `ctrl+,` | Settings |
| `esc` | Back |
| `tab` | Move to the next part of the screen |
| `ctrl+q` | Quit |

Clicking a column title sorts by it, and the boundary between the table and the details can be
dragged.

### Settings

The settings page saves every change at once. The settings are in `~/.config/quvyta/packages.conf` (or under `$XDG_CONFIG_HOME`), next to
the other Quvyta applications' settings. Releases up to 0.1.1 kept them in
`~/.config/quvyta-packages/settings.toml`; the first start of 0.1.2 moves that folder over, and
a file already in the new place is never overwritten.

```toml
[sources]
flatpak = false   # turn a source off; every source is on by default

[aur]
helper = "auto"   # "auto", "paru" or "yay"; auto takes paru when both are installed

[privilege]
tool = "auto"     # "auto", "pkexec" or "sudo"; auto takes pkexec when polkit is installed
```

## Trying it in a container

qpac installs and removes real packages, so the safest way to try it is on a system you can
throw away. From a clone, `run.sh` builds it and opens it in a fresh Arch Linux container with
[podman](https://podman.io); everything it installs stays in the container and is gone when you
quit.

```sh
git clone https://github.com/quvyta/packages
cd packages
./run.sh            # open qpac in the container
./run.sh --shell    # a shell in the same container instead
```

Downloads are cached in two podman volumes, so the second run is quick. `./run.sh --help` says
how to remove them.

## Building from source

The repository holds two crates: `crates/qpackages`, the application, and
`crates/qpackages-core`, published as `quvyta-packages-core`, which reads pacman's files and
plans its transactions without drawing anything or asking for privileges. The toolchain is
pinned by `rust-toolchain.toml`.

```sh
cargo run --bin qpac
```

The tests never run a real package manager: pacman's output is played back from recordings.
Before your first commit, enable the checks (formatting, clippy, tests and docs):

```sh
git config core.hooksPath .githooks
```

## Licence

MIT. See [LICENSE](LICENSE).

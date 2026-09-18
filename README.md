# qpackages

**quvyta-packages** is a package manager for Arch Linux that runs in the terminal. It is meant to
bring pacman, the AUR, Flatpak and Snap together in one simple interface, and shows exactly what
will change before anything does. It is part of the Quvyta family of terminal applications, is
built on [quvyta-framework](https://github.com/quvyta/framework) and is open source under the MIT
licence.

> **Beta.** qpackages is new, and this first release manages pacman's packages only: the AUR,
> Flatpak and Snap are recognised but not yet managed. The interface may still change between
> releases. Please report anything that looks wrong at
> <https://github.com/quvyta/packages/issues>.

## What it does

- **Installed packages.** Every package pacman has installed, in a table you can search and sort,
  with a detail panel: version, installed size, whether you asked for it or it came as a
  dependency, install date, licences, dependencies and website. The list is read straight from
  pacman's local database, so it opens at once and needs no privileges.
- **Removal you can see.** Check one or more packages and remove them. Before anything runs,
  pacman is asked what the removal would do, and the full list, dependencies included, is shown
  for you to confirm. Cancel has the focus, so the safe answer is the default.
- **Your password stays with sudo.** When a password is needed, sudo asks for it on the terminal
  itself; qpackages never sees it. pacman then runs with its output and progress in a pane below
  the list, which stays where it is. A long transaction can be stopped.
- **A locked database is explained, never forced.** If another transaction holds pacman's lock,
  qpackages says so, and by which process when it can tell. It never removes the lock.
- **Sources.** The sidebar shows which sources this machine has. A missing one is shown faint
  with the reason, and where its program is in the official repositories (`paru` for the AUR,
  `flatpak` for Flatpak) it can be installed from there, through the same confirmation. The AUR
  is refused when qpackages runs as root, because packages are never built as root.

The interface follows your system language (English and Turkish are included) and uses the
family's themes, icons, keys and mouse behaviour.

### Not yet

- Installing and upgrading packages from the repositories, and checking for updates.
- Managing AUR, Flatpak and Snap packages. The AUR works through `paru` or `yay`; Snap's own
  package lives in the AUR, so it waits for AUR support.

## Requirements

- Arch Linux, or a distribution built on it, with `pacman`.
- `sudo`, for anything that changes the system.
- Rust 1.95 or later to build it.

## Install

```sh
cargo install quvyta-packages
qpackages
```

The program is installed as `qpackages` and also as `quvyta-packages`.

## Using it

Run `qpackages` without `sudo`: it asks for privileges only when a change is confirmed.

| Key | What it does |
|---|---|
| `/` | Search the packages |
| `space` | Check or uncheck the selected package |
| `delete` | Remove the checked packages, after a confirmation |
| `tab` | Move to the next part of the screen |
| `ctrl+q` | Quit |

Clicking a column title sorts by it, and the boundary between the table and the details can be
dragged.

### Settings

The settings are in `~/.config/quvyta-packages/settings.toml` (or under `$XDG_CONFIG_HOME`):

```toml
[sources]
flatpak = false   # hide a source from the sidebar; every source is shown by default

[aur]
helper = "auto"   # "auto", "paru" or "yay"; auto takes paru when both are installed
```

## Trying it in a container

qpackages installs and removes real packages, so the safest way to try it is on a system you can
throw away. From a clone, `run.sh` builds it and opens it in a fresh Arch Linux container with
[podman](https://podman.io); everything it installs stays in the container and is gone when you
quit.

```sh
git clone https://github.com/quvyta/packages
cd packages
./run.sh            # open qpackages in the container
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
cargo run --bin qpackages
```

The tests never run a real package manager: pacman's output is played back from recordings.
Before your first commit, enable the checks (formatting, clippy, tests and docs):

```sh
git config core.hooksPath .githooks
```

## Licence

MIT. See [LICENSE](LICENSE).

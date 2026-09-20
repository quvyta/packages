# Contributing to qpac

Thank you for taking the time. Bug reports, ideas and pull requests are all welcome.

## Reporting a bug or asking for something

Open an issue at <https://github.com/quvyta/packages/issues> and pick the template that fits. For a bug, the source involved (the repositories, the AUR, Flatpak or Snap), what was on the screen and your distribution usually decide where the problem is, so the template asks for them. Please check that nothing private (host names, user names, keys, addresses, the names of packages you would rather not share) shows in what you paste before you attach it.

Something you would like qpac to do is welcome as a feature request: say what you do with `pacman`, `paru`, `flatpak` or `snap` by hand today, and what you would expect to see before it happens.

## Building and testing

The toolchain is pinned by `rust-toolchain.toml`; `rustup` picks it up by itself.

```sh
git clone https://github.com/quvyta/packages
cd packages
git config core.hooksPath .githooks   # once: formatting, clippy, tests and docs before every commit
cargo test
```

The tests never install, remove or upgrade anything. Every package manager qpac speaks to is reached through one layer that the tests replace with a double, and the answers come from recorded output kept as fixtures, so `cargo test` is safe anywhere and needs no `sudo`. Reading the local package database is the one thing the tests may do for real.

To try the screen, run `cargo run --bin qpac`. It only reads your machine until you confirm something; confirming an install, a removal or an update changes your system for real, so try those on a machine you can afford to repair, such as a virtual machine or a container.

If you need to see how a real `pacman`, `paru`, `flatpak` or `snapd` behaves, measure it in a throwaway container and add what you learned as a fixture. Do not measure on a machine you care about, and never add a test that needs a password.

## Pull requests

- Keep one change per pull request, and say in the description what it changes for the person using qpac.
- The commit hook must pass: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` and `cargo doc` without warnings. Please do not skip it.
- A bug fix comes with a test that fails without it.
- Anything that installs, removes or upgrades shows the full list of what will change and asks once before it starts. A change that would take that away will not be merged.
- A source that is not installed on the machine (say there is no `snap`) is a designed state, not an error: it appears inactive with an offer to install it.
- Text the person reads lives in `crates/qpackages/assets/locales/`, never in the code. Add the English line; if you do not speak the other languages, say so in the pull request and leave them to a later change.
- qpac never runs as root and never handles a password: privileged steps go through `pkexec` and the helper, or through `sudo` in the terminal. A change that would break that will not be merged.
- The interface comes from [quvyta-framework](https://github.com/quvyta/framework). A widget or behaviour every Quvyta application would need belongs there; open an issue in that repository first.

## Licence

By contributing you agree that your contribution is licensed under the MIT licence of this repository.

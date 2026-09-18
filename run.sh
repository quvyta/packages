#!/bin/sh
# Tries qpackages in a clean Arch Linux container with podman.
#
# The application installs and removes real packages, so trying it on the machine you work
# on is a bad idea. This script builds and runs it inside a throwaway Arch container instead:
# everything it installs lands in the container and disappears with it.
#
# Usage:
#   ./run.sh            build and open the application in the container
#   ./run.sh --shell    open a shell in the same container instead, to poke around
#   ./run.sh --help     print this text
#
# Downloads are cached in two named volumes so the second run is fast: packages in
# qpackages-try-cache and the Rust toolchain in qpackages-try-home. To start from scratch:
#   podman volume rm qpackages-try-cache qpackages-try-home
set -eu

usage() {
    sed -n '2,/^set -eu$/{/^set -eu$/d;s/^# \{0,1\}//p}' "$0"
}

mode=app
for arg in "$@"; do
    case "$arg" in
        -h|--help) usage; exit 0 ;;
        --shell) mode=shell ;;
        *) echo "run.sh: unknown argument '$arg'; try --help" >&2; exit 2 ;;
    esac
done

command -v podman >/dev/null 2>&1 || {
    echo "podman is not installed; on Arch run: pacman -S podman" >&2
    exit 1
}

repo=$(cd "$(dirname "$0")" && pwd -P)

# During development the application depends on the framework by path, one directory up.
# A clone of the public repository uses the published crate instead and has no such
# directory, so the mount is added only when the checkout is really there.
framework=""
if [ -f "$repo/../framework/crates/quvyta-framework/Cargo.toml" ]; then
    framework=$(cd "$repo/../framework" && pwd -P)
fi

# Everything that happens inside the container. The image ships without Rust; rustup is
# preferred over the distribution's rust package because the repository pins its toolchain
# in rust-toolchain.toml, and rustup installs exactly that version.
#
# The build writes to a separate target directory: the host's target/ holds artifacts built
# by the host toolchain, and mixing the two would make cargo rebuild everything on both sides.
setup='
set -eu
pacman -Syu --noconfirm --needed rustup base-devel
cd /work/packages
rustup toolchain install "$(sed -n "s/^channel = \"\(.*\)\"/\1/p" rust-toolchain.toml)"
export CARGO_TARGET_DIR=/work/packages/target/container
'
case "$mode" in
    app) command="$setup"'exec cargo run --bin qpackages' ;;
    shell) command="$setup"'exec bash' ;;
esac

# Rootless podman maps the container's root to the host user, so the files cargo writes into
# the mounted repository are owned by you, not by root. Being root inside is what lets pacman
# install into the container. The framework is mounted read-only: the build only reads it.
set -- \
    --rm --interactive --tty \
    --name qpackages-try --replace \
    --env TERM="${TERM:-xterm-256color}" \
    --env LANG=C.UTF-8 \
    --volume "$repo:/work/packages" \
    --volume qpackages-try-cache:/var/cache/pacman/pkg \
    --volume qpackages-try-home:/root
if [ -n "$framework" ]; then
    set -- "$@" --volume "$framework:/work/framework:ro"
fi

exec podman run "$@" docker.io/library/archlinux:latest sh -c "$command"

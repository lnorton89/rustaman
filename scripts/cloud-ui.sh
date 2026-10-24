#!/usr/bin/env bash
#
# Runs this crate's Windows half — tests and screenshots, the GUI
# included — on a Linux machine, for working on the UI from a cloud
# container or any other box without Windows on it.
#
# It is not an emulator of the app's real conditions and is not meant to
# be; see docs/CLOUD_TESTING.md for what it does and does not prove.
#
#   scripts/cloud-ui.sh setup            # install what the rest needs
#   scripts/cloud-ui.sh test             # the whole Windows test suite
#   scripts/cloud-ui.sh test chrome::    # ...or one module of it
#   scripts/cloud-ui.sh shot cpu         # render one scene to a PNG
#   scripts/cloud-ui.sh shot --all
#   scripts/cloud-ui.sh scenes           # what there is to render
#
set -euo pipefail

# The GNU toolchain rather than MSVC: `rustup target add
# x86_64-pc-windows-msvc` installs a standard library but no linker, and
# there is no MSVC linker to be had here. The `-gnu` target links with
# mingw-w64, which apt has. The two targets compile the same source —
# `windows-sys` supports both — so what runs here is the same code the
# `windows` CI job builds, not a port of it.
TARGET=x86_64-pc-windows-gnu
PREFIX="${RUSTAMAN_WINEPREFIX:-${TMPDIR:-/tmp}/rustaman-wine}"

setup() {
    # lavapipe (`mesa-vulkan-drivers`) is the software Vulkan driver that
    # gives wgpu an adapter with no GPU in the machine. Without it the
    # screenshot example aborts with "No adapter found" — the tests still
    # run, because egui_kittest only needs a device when something asks
    # it to rasterise.
    # A container usually runs this as root, where sudo may be absent.
    local sudo=""
    if [ "$(id -u)" -ne 0 ]; then
        sudo=sudo
    fi
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq mesa-vulkan-drivers mingw-w64 wine64 xvfb
    rustup target add "$TARGET"
}

environment() {
    export WINEPREFIX="$PREFIX"
    # Wine's own chatter, on a run whose output is test results.
    export WINEDEBUG=-all
    # A `-gnu` binary links against libgcc and libwinpthread, which live
    # on the host rather than in the prefix.
    export WINEPATH="$(ls -d /usr/lib/gcc/x86_64-w64-mingw32/*-posix 2>/dev/null | head -1)"
    # The app pins wgpu to D3D12, which Wine does not implement. This is
    # the override the pin deliberately leaves open.
    export WGPU_BACKEND=vulkan
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine
    mkdir -p "$PREFIX"
}

# Wine's Vulkan driver wants a display connection even to enumerate an
# adapter for an offscreen render, so anything that rasterises is run
# under a virtual one. The tests are not, because they do not need it.
with_display() {
    xvfb-run -a "$@"
}

case "${1:-}" in
    setup)
        setup
        ;;
    test)
        shift
        environment
        cargo test --target "$TARGET" --lib "$@"
        ;;
    shot)
        shift
        environment
        # A bare scene name is the common case, so spell `--scene` for
        # the caller. Anything already starting with a dash — `--all`,
        # `--size`, `--theme` — is passed through untouched.
        if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then
            set -- --scene "$@"
        fi
        with_display cargo run --quiet --target "$TARGET" \
            --example screenshot -- "$@"
        ;;
    scenes)
        environment
        cargo run --quiet --target "$TARGET" --example screenshot -- --list
        ;;
    *)
        sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
        exit 64
        ;;
esac

#!/bin/sh
set -eu

PATH="$HOME/.cargo/bin:$PATH:/opt/homebrew/bin:/usr/local/bin"
export PATH
action="${1:-check}"
os="$(uname -s)"

fail() {
    printf '%s\nRun make requirements before building/installing.\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "Missing build tool: $1"
}

as_root() {
    if [ "$(id -u)" = 0 ]; then
        "$@"
    else
        need sudo
        sudo "$@"
    fi
}

have_rust() {
    command -v "${CARGO:-cargo}" >/dev/null 2>&1 &&
        command -v rustc >/dev/null 2>&1 &&
        rustc --version | awk 'NR == 1 { split($2, v, "."); ok = v[1] > 1 || (v[1] == 1 && v[2] >= 89) } END { exit !ok }'
}

install_packages() {
    case "$os" in
        FreeBSD)
            packages="cmake pkgconf sdl2 x265 aom libde265"
            if ! have_rust; then packages="$packages rust"; fi
            missing=""
            for package in $packages; do
                if ! pkg info -e "$package"; then missing="$missing $package"; fi
            done
            if [ -n "$missing" ]; then as_root pkg install -y $missing; fi
            ;;
        Darwin)
            xcode-select -p >/dev/null 2>&1 ||
                fail "Install Apple's Command Line Tools with xcode-select --install first."
            need brew
            packages="cmake pkgconf sdl2 x265 aom libde265"
            if ! have_rust; then packages="$packages rust"; fi
            missing=""
            for package in $packages; do
                if ! brew list --versions "$package" >/dev/null 2>&1; then missing="$missing $package"; fi
            done
            if [ -n "$missing" ]; then HOMEBREW_NO_AUTO_UPDATE=1 brew install $missing; fi
            ;;
        Linux)
            if command -v apt-get >/dev/null 2>&1; then
                as_root apt-get update
                as_root apt-get install -y --no-install-recommends --no-upgrade \
                    build-essential cmake pkg-config curl ca-certificates \
                    libsdl2-dev libx265-dev libaom-dev libde265-dev aom-tools libnuma-dev
            elif command -v apk >/dev/null 2>&1; then
                as_root apk add build-base cmake pkgconf curl ca-certificates \
                    sdl2-dev x265-dev aom-dev libde265-dev numactl-dev
            else
                fail "Automatic Linux packages require apt-get (Debian/Ubuntu) or apk (Alpine). See README.md."
            fi
            ;;
        *) fail "Unsupported requirements platform: $os" ;;
    esac
}

install_rust() {
    if have_rust; then return; fi
    if ! command -v rustup >/dev/null 2>&1; then
        need curl
        installer="$(mktemp)"
        trap 'rm -f "$installer"' 0
        curl --fail --location --retry 3 --retry-all-errors --output "$installer" https://sh.rustup.rs
        sh "$installer" -y --no-modify-path --profile minimal --component rustfmt,clippy
        rm -f "$installer"
        trap - 0
    else
        rustup toolchain install stable --profile minimal --component rustfmt,clippy
        if ! have_rust; then rustup override set stable; fi
    fi
    have_rust || fail "Rust 1.89+ and Cargo are required; check the active rustup override."
}

check() {
    for tool in "${CARGO:-cargo}" rustc cc c++ cmake pkg-config make; do need "$tool"; done
    have_rust || fail "Rust 1.89+ is required."
    cmake --version | awk 'NR == 1 { split($3, v, "."); ok = v[1] > 3 || (v[1] == 3 && v[2] >= 22) } END { exit !ok }' ||
        fail "CMake 3.22+ is required."
    pkg-config --print-errors --exists sdl2 x265 aom libde265 ||
        fail "Missing SDL2, x265, libaom or libde265 development files."
    case "$os" in
        FreeBSD|Linux)
            pkg-config --print-errors --exists x11 xext xrandr xi xcursor xfixes \
                libdrm gbm egl ||
                fail "Missing X11/DRM/GBM/EGL development files for SDL2."
            ;;
        Darwin) need otool ;;
    esac
}

case "$action" in
    install)
        install_packages
        install_rust
        check
        printf 'Requirements ready. Run make install.\n'
        ;;
    check) check ;;
    *) fail "Usage: requirements.sh install|check" ;;
esac

#!/bin/sh
# Install niri-sidestage from a GitHub release.
#
#   curl -fsSL https://github.com/anisharaz/niri-sidestage/releases/latest/download/install.sh | sh
#
# Environment:
#   NIRI_SIDESTAGE_VERSION  release tag to install, e.g. v0.1.0 (default: latest)
#   PREFIX                  install prefix (default: ~/.local, binary goes to $PREFIX/bin)
#
# Usage:  install.sh [install|uninstall]
set -eu

REPO="anisharaz/niri-sidestage"
BIN="niri-sidestage"
ASSET="niri-sidestage-x86_64-linux"
VERSION="${NIRI_SIDESTAGE_VERSION:-latest}"
PREFIX="${PREFIX:-$HOME/.local}"
DEST="$PREFIX/bin/$BIN"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "${1:-install}" in
    install) ;;
    uninstall)
        if [ -e "$DEST" ]; then
            rm -f "$DEST"
            say "removed $DEST"
        else
            say "nothing to remove at $DEST"
        fi
        exit 0
        ;;
    *) die "usage: $0 [install|uninstall]" ;;
esac

[ "$(uname -s)" = "Linux" ] || die "only Linux is supported"
case "$(uname -m)" in
    x86_64 | amd64) ;;
    *) die "only x86_64 builds are published; build from source instead (see README)" ;;
esac

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q "$1" -O "$2"; }
else
    die "need curl or wget"
fi
command -v sha256sum >/dev/null 2>&1 || die "need sha256sum"

if [ "$VERSION" = "latest" ]; then
    BASE="https://github.com/$REPO/releases/latest/download"
else
    BASE="https://github.com/$REPO/releases/download/$VERSION"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

say "downloading $BIN ($VERSION)..."
fetch "$BASE/$ASSET" "$TMP/$ASSET" || die "could not download $BASE/$ASSET"
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || die "could not download $BASE/SHA256SUMS"

say "verifying checksum..."
(cd "$TMP" && grep " $ASSET\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1) \
    || die "checksum mismatch, refusing to install"

mkdir -p "$PREFIX/bin"
install -m 755 "$TMP/$ASSET" "$DEST"
say "installed $DEST"

# The binary links against GTK 4 and gtk4-layer-shell, which are not bundled.
if command -v ldd >/dev/null 2>&1; then
    MISSING="$(ldd "$DEST" 2>/dev/null | grep 'not found' | awk '{print $1}' || true)"
    if [ -n "$MISSING" ]; then
        say ""
        say "warning: missing shared libraries:"
        printf '  %s\n' $MISSING
        say "install them first:"
        say "  Arch:    sudo pacman -S gtk4 gtk4-layer-shell"
        say "  Fedora:  sudo dnf install gtk4 gtk4-layer-shell"
        say "  Debian:  sudo apt install libgtk-4-1 libgtk4-layer-shell0"
    fi
fi

case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) say ""; say "note: $PREFIX/bin is not on your PATH; use the full path below." ;;
esac

say ""
say "start it with niri by adding this to your niri config:"
say "  spawn-at-startup \"$DEST\""
say "or run it now:  $DEST"

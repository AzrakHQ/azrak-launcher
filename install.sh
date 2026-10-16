#!/usr/bin/env bash
# Azrak Launcher installer: installs the launcher and adds it to your app menu.
# In a release download it installs the included binary; in a source checkout it builds first.
#
#   ./install.sh               install for the current user (no sudo)
#   ./install.sh --autostart   also start the launcher when you log in
#   ./install.sh --uninstall   remove the launcher (keeps your settings and cache)
#   PREFIX=/usr/local sudo -E ./install.sh   system-wide install
set -euo pipefail

APP=azrak
# The name before Azrak: an older install is replaced (the launcher moves its settings itself).
OLD=ps5-launcher
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
[ "$PREFIX" != "$HOME/.local" ] && DATA_DIR="$PREFIX/share"
APPS_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor/scalable/apps"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

distro_hint() {
    # Print the package-manager command for build dependencies on this distro.
    if command -v apt-get >/dev/null; then
        echo "sudo apt install build-essential pkg-config libfontconfig1-dev libxkbcommon-dev"
    elif command -v dnf >/dev/null; then
        echo "sudo dnf install gcc pkgconf-pkg-config fontconfig-devel libxkbcommon-devel"
    elif command -v pacman >/dev/null; then
        echo "sudo pacman -S --needed base-devel fontconfig libxkbcommon"
    elif command -v zypper >/dev/null; then
        echo "sudo zypper install gcc pkg-config fontconfig-devel libxkbcommon-devel"
    else
        echo "install a C compiler, pkg-config and the fontconfig development package"
    fi
}

uninstall() {
    bold "Removing Azrak Launcher"
    for name in "$APP" "$OLD"; do
        rm -f "$BIN_DIR/$name" "$BIN_DIR/$name.previous" "$APPS_DIR/$name.desktop" "$ICON_DIR/$name.svg" "$AUTOSTART_DIR/$name.desktop"
    done
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS_DIR" 2>/dev/null || true
    ok "Removed. Settings and cache are kept in ~/.config/$APP and ~/.cache/$APP (delete them to reset)."
}

AUTOSTART=0
for arg in "$@"; do
    case "$arg" in
        --uninstall) uninstall; exit 0 ;;
        --autostart) AUTOSTART=1 ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) die "unknown option: $arg" ;;
    esac
done

[ "$(uname -s)" = "Linux" ] || die "Azrak Launcher runs on Linux only."

# Release archives ship a prebuilt binary next to this script: nothing to compile.
PREBUILT="$HERE/$APP"
if [ -f "$PREBUILT" ] && [ ! -f "$HERE/Cargo.toml" ]; then
    bold "1/2  Checking the prebuilt binary"
    "$PREBUILT" --version >/dev/null 2>&1 || die "The prebuilt binary does not run on this system (it needs glibc 2.35+, e.g. Ubuntu 22.04 or newer). Build from source instead: https://github.com/AzrakHQ/azrak#build-from-source"
    ok "$("$PREBUILT" --version)"
    BIN_SRC="$PREBUILT"
    STEP="2/2"
else
bold "1/3  Checking tools"
if ! command -v cargo >/dev/null; then
    [ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null; then
    warn "Rust is not installed."
    read -r -p "  Install Rust now with rustup (https://rustup.rs, no sudo needed)? [Y/n] " yn
    case "${yn:-y}" in
        [Yy]*) curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
               export PATH="$HOME/.cargo/bin:$PATH" ;;
        *) die "Rust is required to build the launcher." ;;
    esac
fi
ok "$(cargo --version)"
command -v cc >/dev/null || die "No C compiler found. Install build tools first:  $(distro_hint)"
ok "C compiler found"

bold "2/3  Building (optimized release, takes a minute the first time)"
cd "$HERE"
if ! cargo build --release --locked; then
    echo
    warn "The build failed. The most common cause is a missing system package:"
    echo "      $(distro_hint)"
    warn "If your Rust is older than 1.92, update it:  rustup update stable"
    exit 1
fi
ok "Built target/release/$APP"
BIN_SRC="target/release/$APP"
STEP="3/3"
fi

bold "$STEP  Installing"
cd "$HERE"
mkdir -p "$BIN_DIR" "$APPS_DIR" "$ICON_DIR"
# An install from before the rename: its binary, menu entry and icon go (settings move on first start).
[ -e "$AUTOSTART_DIR/$OLD.desktop" ] && AUTOSTART=1
rm -f "$BIN_DIR/$OLD" "$BIN_DIR/$OLD.previous" "$APPS_DIR/$OLD.desktop" "$ICON_DIR/$OLD.svg" "$AUTOSTART_DIR/$OLD.desktop"
install -m 755 "$BIN_SRC" "$BIN_DIR/$APP"
install -m 644 "assets/$APP.svg" "$ICON_DIR/$APP.svg"
cat > "$APPS_DIR/$APP.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Azrak Launcher
GenericName=Game Launcher
Comment=Console-style game launcher for Linux
Exec=$BIN_DIR/$APP
Icon=$APP
Terminal=false
Categories=Game;
Keywords=azrak;ps5;ps4;playstation;kyty;shadps4;emulator;games;
StartupWMClass=$APP
EOF
ok "Binary      $BIN_DIR/$APP"
ok "Menu entry  $APPS_DIR/$APP.desktop"
if [ "$AUTOSTART" = 1 ]; then
    mkdir -p "$AUTOSTART_DIR"
    cp "$APPS_DIR/$APP.desktop" "$AUTOSTART_DIR/$APP.desktop"
    ok "Autostart   starts when you log in"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -f -t "$DATA_DIR/icons/hicolor" 2>/dev/null || true

echo
bold "Optional helpers"
for tool in xdotool:"Resume/Stop and PS-button switching between game and launcher" \
            mpv:"fullscreen trailers" yt-dlp:"YouTube trailers (with mpv)" xrandr:"display detection"; do
    name="${tool%%:*}"; why="${tool#*:}"
    if command -v "$name" >/dev/null; then ok "$name — $why"; else warn "$name not found — needed for $why"; fi
done
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not in your PATH; start it from the app menu or run $BIN_DIR/$APP" ;;
esac
echo
bold "Done! Start Azrak Launcher from your app menu, or run:  $APP"

#!/usr/bin/env bash
# Build "PS5 Launcher.app" from the two per-architecture binaries and zip it for release.
#
#   packaging/macos/bundle.sh <arm64-binary> <x86_64-binary> <out-dir>
#
# Set SKIP_LICENSES=1 to leave out the third-party notices.
# Produces <out-dir>/ps5-launcher-macos-universal/PS5 Launcher.app plus
# <out-dir>/ps5-launcher-macos-universal.zip and .zip.sha256.
# The app is ad-hoc signed (no Apple Developer account needed), so Gatekeeper asks the user to
# approve it once: right-click -> Open, or `xattr -dr com.apple.quarantine "PS5 Launcher.app"`.
set -euo pipefail

[ "$(uname)" = Darwin ] || { echo "error: run this on macOS (needs lipo, codesign, ditto)" >&2; exit 1; }
[ $# -eq 3 ] || { echo "usage: $0 <arm64-binary> <x86_64-binary> <out-dir>" >&2; exit 1; }

ARM64="$1"; X86_64="$2"; OUT="$3"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
NAME=ps5-launcher-macos-universal
STAGE="$OUT/$NAME"
APP="$STAGE/PS5 Launcher.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || { echo "error: could not read the version from Cargo.toml" >&2; exit 1; }

rm -rf "$STAGE" "$OUT/$NAME.zip" "$OUT/$NAME.zip.sha256"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [ "$ARM64" = "$X86_64" ]; then
    # Same file for both (CI smoke test): lipo refuses duplicate architectures, so just copy.
    cp "$ARM64" "$APP/Contents/MacOS/ps5-launcher"
else
    lipo -create "$ARM64" "$X86_64" -output "$APP/Contents/MacOS/ps5-launcher"
fi
chmod 755 "$APP/Contents/MacOS/ps5-launcher"

sed "s/@VERSION@/$VERSION/g" "$HERE/Info.plist" > "$APP/Contents/Info.plist"

# App icon: needs rsvg-convert (brew install librsvg) and iconutil. Without them the app still works.
if command -v rsvg-convert >/dev/null && command -v iconutil >/dev/null; then
    ICONSET="$(mktemp -d)/ps5-launcher.iconset"
    mkdir -p "$ICONSET"
    for size in 16 32 128 256 512; do
        rsvg-convert -w "$size" -h "$size" "$ROOT/assets/ps5-launcher.svg" -o "$ICONSET/icon_${size}x${size}.png"
        rsvg-convert -w "$((size * 2))" -h "$((size * 2))" "$ROOT/assets/ps5-launcher.svg" -o "$ICONSET/icon_${size}x${size}@2x.png"
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/ps5-launcher.icns"
else
    echo "note: rsvg-convert/iconutil not found; building without an app icon" >&2
fi

# Ad-hoc signature ("-"): required for arm64 binaries to run at all.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

cp "$ROOT/README.md" "$STAGE/"
# SKIP_LICENSES=1 skips the third-party notices (they need network access to GitHub).
if [ -z "${SKIP_LICENSES:-}" ] && [ -f "$ROOT/scripts/package-licenses.mjs" ] && command -v node >/dev/null; then
    # Both Mac architectures pull in the same crates; list the ones for Apple Silicon.
    PS5_LICENSE_TARGET=aarch64-apple-darwin node "$ROOT/scripts/package-licenses.mjs" "$STAGE" --allow-upstream
fi

# ditto keeps the bundle's permissions and structure intact (plain zip can break them).
ditto -c -k --keepParent "$STAGE" "$OUT/$NAME.zip"
(cd "$OUT" && shasum -a 256 "$NAME.zip" > "$NAME.zip.sha256")
echo "built $OUT/$NAME.zip (version $VERSION)"
lipo -archs "$APP/Contents/MacOS/ps5-launcher"

#!/usr/bin/env bash
# Builds ps5-launcher-linux-x86_64.AppImage from the release binary. The file name is the one the
# launcher's own updater looks for. Needs curl and sha256sum, and downloads appimagetool (pinned).
#
#   packaging/linux/build-appimage.sh target/release/ps5-launcher dist/packages
set -euo pipefail

binary="${1:?usage: $0 <binary> <output folder>}"
out="${2:?usage: $0 <binary> <output folder>}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Pinned with its checksum. Dependabot does not follow this, so update both by hand.
tool_version=1.9.1
tool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0

work="$(mktemp -d)"
trap 'rm -r "$work"' EXIT

# APPIMAGETOOL=/path/to/appimagetool uses that tool and skips the download (for testing on another
# CPU); ARCH then names the CPU of the binary.
if [ -n "${APPIMAGETOOL:-}" ]; then
  cp "$APPIMAGETOOL" "$work/appimagetool"
else
  curl -fsSL -o "$work/appimagetool" \
    "https://github.com/AppImage/appimagetool/releases/download/$tool_version/appimagetool-x86_64.AppImage"
  echo "$tool_sha256  $work/appimagetool" | sha256sum -c -
fi
chmod +x "$work/appimagetool"

app="$work/PS5Launcher.AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/icons/hicolor/scalable/apps" "$out"
install -m 755 "$binary" "$app/usr/bin/ps5-launcher"
install -m 755 "$root/packaging/linux/appimage/AppRun" "$app/AppRun"
install -m 644 "$root/packaging/linux/ps5-launcher.desktop" "$app/ps5-launcher.desktop"
install -m 644 "$root/assets/ps5-launcher.svg" "$app/ps5-launcher.svg"
install -m 644 "$root/assets/ps5-launcher.svg" "$app/usr/share/icons/hicolor/scalable/apps/ps5-launcher.svg"
ln -s ps5-launcher.svg "$app/.DirIcon"

# appimagetool is an AppImage itself; extract-and-run avoids needing FUSE on the build machine.
file="$out/ps5-launcher-linux-x86_64.AppImage"
ARCH="${ARCH:-x86_64}" APPIMAGE_EXTRACT_AND_RUN=1 "$work/appimagetool" "$app" "$file"
chmod 755 "$file"
(cd "$out" && sha256sum "$(basename "$file")" > "$(basename "$file").sha256")
ls -la "$out"

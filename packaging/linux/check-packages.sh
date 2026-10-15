#!/usr/bin/env bash
# Checks the packages built from packaging/linux/nfpm.yaml that are in a folder: every file is in
# each package, the programs are executable, and the dependency lists are there. Any mix of deb, rpm
# and Arch packages is fine, but there must be at least one. Needs dpkg-deb (deb), rpm (rpm) and
# tar with zstd (Arch).
#
#   packaging/linux/check-packages.sh dist/packages
set -euo pipefail

dir="${1:?usage: $0 <folder with the packages>}"
shopt -s nullglob
debs=("$dir"/*.deb); rpms=("$dir"/*.rpm); arch=("$dir"/*.pkg.tar.zst)
[ $((${#debs[@]} + ${#rpms[@]} + ${#arch[@]})) -gt 0 ] || { echo "no deb, rpm or Arch package in $dir" >&2; exit 1; }
for list in debs rpms arch; do
  declare -n files="$list"
  [ ${#files[@]} -le 1 ] || { echo "more than one $list package in $dir" >&2; exit 1; }
done

want=(
  usr/bin/ps5-launcher
  usr/bin/ps5-launcher-session
  usr/share/applications/ps5-launcher.desktop
  usr/share/wayland-sessions/ps5-launcher.desktop
  usr/share/icons/hicolor/scalable/apps/ps5-launcher.svg
)
fail=0
need() { # <package kind> <listing> <path>
  grep -Eq "(^|[ ./])$3\$" <<<"$2" || { echo "$1 is missing /$3" >&2; fail=1; }
}
checked=()

if [ ${#debs[@]} -eq 1 ]; then
  list=$(dpkg-deb --contents "${debs[0]}")
  for f in "${want[@]}"; do need deb "$list" "$f"; done
  # The two programs must be executable.
  for f in usr/bin/ps5-launcher usr/bin/ps5-launcher-session; do
    grep -E "^-rwxr-xr-x .*[ ./]$f\$" <<<"$list" >/dev/null || { echo "deb: /$f is not 0755" >&2; fail=1; }
  done
  # Dependencies the launcher loads at run time by name.
  info=$(dpkg-deb --info "${debs[0]}")
  for dep in libarchive13 xdotool; do grep -q "$dep" <<<"$info" || { echo "deb does not depend on $dep" >&2; fail=1; }; done
  checked+=("$(basename "${debs[0]}")")
fi

if [ ${#rpms[@]} -eq 1 ]; then
  list=$(rpm -qpl "${rpms[0]}" | sed 's#^/##')
  for f in "${want[@]}"; do need rpm "$list" "$f"; done
  req=$(rpm -qp --requires "${rpms[0]}")
  for dep in libarchive xdotool; do grep -q "$dep" <<<"$req" || { echo "rpm does not require $dep" >&2; fail=1; }; done
  checked+=("$(basename "${rpms[0]}")")
fi

if [ ${#arch[@]} -eq 1 ]; then
  list=$(tar --zstd -tf "${arch[0]}")
  for f in "${want[@]}"; do need arch "$list" "$f"; done
  checked+=("$(basename "${arch[0]}")")
fi

[ "$fail" = 0 ] && echo "packages look right: ${checked[*]}"
exit "$fail"

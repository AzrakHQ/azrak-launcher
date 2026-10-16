#!/usr/bin/env bash
# Removal matrix: the app must build, start and pass its tests with no extensions, each alone, and each removed.
# Cells are feature sets of the `launcher` binary. Add one line per extension feature.
set -euo pipefail
cd "$(dirname "$0")/.."
FEATURES=(native-pc)          # every extension feature, one entry each
run() { echo "== launcher features: [${1:-none}]"; cargo test -q -p launcher --no-default-features ${1:+--features "$1"}; }
run ""                                    # nothing installed
for f in "${FEATURES[@]}"; do run "$f"; done   # each alone
all="${FEATURES[*]}"; all="${all// /,}"
run "$all"                                # everything
for f in "${FEATURES[@]}"; do             # everything except one
  rest=$(printf "%s\n" "${FEATURES[@]}" | { grep -vx "$f" || true; } | paste -sd, -)
  run "$rest"
done
run "toy"                                 # the toy plugin alone
echo "removal matrix passed"

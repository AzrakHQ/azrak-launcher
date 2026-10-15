#!/usr/bin/env bash
# Run the built launcher far enough to prove it starts: `--version` must report Cargo.toml's version.
set -euo pipefail
exe=${1:?usage: smoke-test.sh <path to ps5-launcher>}
root=$(cd "$(dirname "$0")/.." && pwd)
expected=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
actual=$("$exe" --version)
echo "reports: $actual (expected $expected)"
case "$actual" in *"$expected") ;; *) echo "unexpected version output" >&2; exit 1 ;; esac

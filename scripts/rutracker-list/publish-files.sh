#!/usr/bin/env bash
# Put the file lists files.mjs collected into the published catalog's Russian sources (the
# `catalog` branch), then start the daily workflow, which translates them and publishes the English
# catalogs launchers read. A list goes to the same topic only while its torrent is the same.
#
#   scripts/rutracker-list/publish-files.sh [dist/rutracker]
set -euo pipefail
here=$(dirname "$(realpath "$0")")
src=$(realpath "${1:-dist/rutracker}")
remote=$(git -C "$here" remote get-url origin)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git clone -q --depth 1 --branch catalog "$remote" "$work/catalog"
for p in linux wine; do
  [ -f "$src/$p-topics.json.ru-backup.json" ] && [ -f "$work/catalog/$p-topics.ru.json.gz" ] || continue
  gunzip -c "$work/catalog/$p-topics.ru.json.gz" >"$work/$p.json"
  node "$here/files.mjs" --merge "$src/$p-topics.json.ru-backup.json" "$work/$p.json"
  gzip -9nc "$work/$p.json" >"$work/catalog/$p-topics.ru.json.gz"
done
cd "$work/catalog"
if git diff --quiet; then echo "Nothing new to publish"; exit 0; fi
git -c user.name="catalog bot" -c user.email="actions@users.noreply.github.com" commit -qam "File lists $(date -u +%F)"
git push -q origin catalog
gh workflow run catalog.yml --repo "$(git remote get-url origin | sed -E 's#.*github.com[:/]##; s#\.git$##')"
echo "Published; the catalog workflow is translating them now."

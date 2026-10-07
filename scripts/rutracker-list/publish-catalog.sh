#!/usr/bin/env bash
# Publish the catalogs to the `catalog` branch, which the launcher downloads every 6 hours:
# gzipped English catalogs, the Russian sources and translation cache the next refresh starts
# from, and catalog.json (SHA-256 of each catalog). The branch keeps one commit, replaced each time.
#
#   scripts/rutracker-list/publish-catalog.sh [dist/rutracker] [git remote URL]
set -euo pipefail
src=$(realpath "${1:-dist/rutracker}")
remote=${2:-$(git remote get-url origin)}
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
for p in ps5 ps4; do
  gzip -9nc "$src/$p-topics.json" >"$out/$p-topics.json.gz"
  gzip -9nc "$src/$p-topics.json.ru-backup.json" >"$out/$p-topics.ru.json.gz"
done
gzip -9nc "$src/english-translations.json" >"$out/english-translations.json.gz"
node - "$src" >"$out/catalog.json" <<'JS'
const { createHash } = require('node:crypto');
const { readFileSync } = require('node:fs');
const files = {};
for (const name of ['ps5-topics.json', 'ps4-topics.json']) {
  const bytes = readFileSync(`${process.argv[2]}/${name}`);
  files[name] = { sha256: createHash('sha256').update(bytes).digest('hex'), bytes: bytes.length,
    topics: JSON.parse(bytes).topic_count };
}
console.log(JSON.stringify({ refreshed_at: new Date().toISOString(), files }, null, 2));
JS
cat >"$out/README.md" <<'MD'
# Game catalog

Published daily by `.github/workflows/catalog.yml`; the launcher downloads `catalog.json` and the
`*-topics.json.gz` files that changed. Don't commit here: each refresh replaces this branch.
MD
cd "$out"
git init -q -b catalog
git add .
git -c user.name="catalog bot" -c user.email="actions@users.noreply.github.com" commit -qm "Game catalog $(date -u +%F)"
git push -qf "$remote" catalog
echo "Published $(jq -r '.files | to_entries | map("\(.key): \(.value.topics)") | join(", ")' catalog.json)"

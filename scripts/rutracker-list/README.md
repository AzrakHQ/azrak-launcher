# RuTracker topic collector (PS5 and PS4)

Every script takes `--platform ps5|ps4` (default `ps5`): PS5 is forum 546, PS4 is forum 973.
Output, translation and publishing use `<platform>-topics.json`. For PS4:

```bash
node list.mjs --platform ps4 --include-magnets
node translate.mjs --platform ps4
node publish.mjs --platform ps4      # bundles assets/rutracker/ps4-topics.json
```

The dedicated public PS5 forum is
<https://rutracker.net/forum/viewforum.php?f=546>. Collect its **listing pages**
instead of relying on the authenticated/blocked `tracker.php` search endpoint.

## Verified result

On 2026-09-30, all **13 listing pages** were visited in an already accessible
normal browser. The export contains **614 distinct topic IDs**, excluding the
five untagged pinned threads. The generated catalog is available in
[../../dist/rutracker/ps5-topics.json](../../dist/rutracker/ps5-topics.json).
This is a snapshot of release topics, not 614 distinct games.

The same snapshot was enriched on 2026-09-30: **614/614 official magnet links**
and **614/614 main-post game-information records** were collected. Both
`magnet_collection.complete` and `game_info_collection.complete` are true, with
no failures or remaining topics. Original listing metadata and topic order were
preserved. A pre-enrichment backup is retained next to the generated catalog.

The local parser tests pass. A separate live run with this tool's fresh Chrome
profile encountered Cloudflare verification and correctly produced a blocked
partial report. The successful snapshot used the accessible browser; it does
**not** prove that new profiles or unattended refreshes will work.

## Refreshing the catalog

This standalone Node tool uses an ordinary, visible Chrome browser. If Cloudflare
verification or login appears, complete it **yourself in the browser**. There is
no challenge solver, stealth plugin, credential collection or cookie export.
Google Chrome must be installed. Requires Node 20+.

From this directory:

```sh
npm install
npm test
npm run list
```

It follows the forum's pagination, reads topic titles, IDs, public URLs, sizes,
authors and peer counts, and deduplicates by topic ID. Rules/discussion threads
without a `[PS5]` prefix are excluded. It does **not** open topic pages, fetch
magnets/torrents, or download/extract/launch any games.

### Optional magnet and game information metadata

Use `npm run list -- --include-magnets` to additionally read each topic's official
magnet link and factual release details from its main post. To enrich an existing
listing without crawling its pagination again:

```sh
npm run list -- --enrich-existing ../../dist/rutracker/ps5-topics.json
```

The tool reads same-origin topic HTML sequentially, with a 1.5-second delay;
it never clicks a magnet, fetches a torrent, contacts a tracker/peer, or downloads
a game. It stops at verification, login or HTTP failures without bypassing them.
Each topic receives `magnet` and `magnet_status` fields. Unknown links remain
`null` rather than being guessed. `magnet_collection` records coverage separately
from `complete`, which still describes listing coverage. A blocked/partial
enrichment is saved separately, preserving the original catalog.

`game_info` contains normalized labelled facts such as genre, developer,
publisher, release year/date, title ID, region, edition, version, image format,
firmware, languages and age rating. Original factual labels are retained in
`fields`; image URLs are metadata only and are not downloaded. A short
`release_summary` is generated from those facts. Long promotional descriptions,
forum replies and installation instructions inside spoilers are not copied.
`game_info_status` and `game_info_collection` distinguish missing/inaccessible
details from successfully extracted information. Release-test/firmware claims
come from the uploader and are not emulator compatibility guarantees.

Default output is `dist/rutracker/ps5-topics.json` relative to the launcher root.
`--output /path/to/catalog.json`, `--max-pages 200`, and
`--verification-timeout 180` are supported. Use `--headless` only for diagnostics;
verification cannot be completed interactively there. Exit 0 means all discovered
forum listing pages were visited. Exit 2 means partial/blocked. Partial results
go to a separate `.partial.json` file, preserving any previous complete catalog.

The dedicated browser profile lives in
`~/.cache/azrak/rutracker-list-browser`. It may contain your session cookies;
keep it private and do not commit or share it. Only one collector can use that
profile at a time. The tool requests pages sequentially with a 1.5-second delay
and stops on access failures instead of retrying aggressively.

## Scope and limitations

### English catalog

The generated catalog was translated to English on 2026-09-30. All 614 records
retain their original IDs, magnet links, URLs, image URLs and peer statistics.
Russian factual labels and text were translated with a reviewed terminology
glossary and Google Translate; technical versions and dates are protected against
number reformatting. Run-on extraction boundaries were cleaned, and no Russian
labels or human-readable Russian values remain in the English catalog.

The original enriched Russian snapshot is preserved next to the catalog as
`ps5-topics.json.ru-backup.json`. To translate a freshly collected Russian catalog,
run `npm run translate`. The translator sends only deduplicated public metadata
text to Google's translation endpoint, not credentials, cookies or the entire
JSON. Dedicated URL/magnet fields are excluded; any public link embedded in a
metadata sentence is preserved. Translations are cached locally. `npm run translate -- --from-original`
regenerates English from the preserved Russian backup rather than the current file.
Uploader-provided claims are translated, not independently verified.

- A topic is a release listing, **not necessarily a distinct game**. Multiple
  regions, versions, formats and editions can have separate topics.
- `complete` means coverage of the pagination discovered in forum 546 during
  that run. It does not claim an atomic snapshot, deleted/private topics, other
  forums, or untagged releases. New posts can move between pages during collection.
- Missing peer counts are `null`, not zero. Statistics change continuously.
- Forum access may require human verification or login. Automation cannot guarantee
  unattended access. No credentials should be supplied through chat.
- The Rust/Slint launcher consumes the English JSON locally. Browser collection
  remains separate; **Reload RuTracker catalog** only reloads the file.

### Daily refresh without a browser

[refresh.mjs](refresh.mjs) updates a source (Russian) catalog from RuTracker's public endpoints,
which answer plain requests: the forum's Atom feed (`feed.rutracker.cc/atom/f/<forum>.atom`,
titles of new and re-uploaded releases from the last 1–4 weeks) and the API
(`api.rutracker.cc/v1/static/pvc/f/<forum>`, every release's hash, size and peers).
[api.mjs](api.mjs) applies only the API to an existing catalog. Topics the API no longer lists
are dropped, unless it answered for under 80% of the catalog. A new topic has no details (its
page isn't read), so it borrows the game's facts (title ID, genre, developer, cover) from another
release of the same game; it's marked `borrowed`, and the browser collector still reads its own. Then
`translate.mjs --from-original` translates the new strings, and
[publish-catalog.sh](publish-catalog.sh) pushes everything to the `catalog` branch the launcher
downloads. [.github/workflows/catalog.yml](../../.github/workflows/catalog.yml) runs these daily.
The branch must exist first: seed it once from a complete local collection
(`dist/rutracker/{ps5,ps4}-topics.json`, their `.ru-backup.json` sources and
`english-translations.json`) with `scripts/rutracker-list/publish-catalog.sh`.

### PC games

`--platform linux` (native Linux games, f=1992) and `--platform wine` (Wine-packed, f=2059) work
with every script. Those forums have no title tag: every topic with a torrent is a release.
[steam.mjs](steam.mjs) adds Steam artwork and ProtonDB ratings to their Russian sources before
translation; the daily workflow runs it on up to 900 releases a day.

On `--platform ps5|ps4` it only looks up releases without a title ID, for which the launcher
can't find PlayStation artwork (mostly new releases from the daily refresh), and only accepts an
exact name match (edition words aside); ProtonDB isn't asked.

### Publishing the offline launcher snapshot

After collection and translation, `npm run publish` validates the complete English
listing and copies it to [../../assets/rutracker/ps5-topics.json](../../assets/rutracker/ps5-topics.json).
Commit that metadata asset with source releases: it is embedded at compile time,
so installed binaries need neither the ignored generated directory nor Chrome.
Publishing copies metadata only; it makes no network requests and starts no transfers.
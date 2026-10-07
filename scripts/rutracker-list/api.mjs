#!/usr/bin/env node
// Magnets and peer counts for every topic of a forum from RuTracker's public API, in one request
// (https://api.rutracker.cc/v1/static/pvc/f/<forum>): no topic pages, no login. Titles still come
// from the forum listing (list.mjs), and release details from topic pages (--include-magnets).
import { readFile, writeFile, rename } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { platformFrom } from './platforms.mjs';

// The tracker RuTracker's own magnet links announce to.
const TRACKER = 'http://bt3.t-ru.org/ann?magnet';

/**
 * Fill magnets and peers from a `pvc` response. Magnets already read from topic pages are kept;
 * with `replaceChanged`, one whose torrent was re-uploaded (a new hash) is replaced.
 */
export function applyApi(catalog, api, { replaceChanged = false } = {}) {
  const fields = api.format.topic_id;
  const at = name => fields.indexOf(name);
  const observed = new Date(api.update_time * 1000).toISOString();
  let matched = 0;
  const topics = catalog.topics.map(topic => {
    const row = api.result[topic.id];
    if (!row) return topic;
    matched++;
    const hash = String(row[at('info_hash')] || '').toUpperCase();
    const valid = /^[A-F\d]{40}$/.test(hash);
    const current = /urn:btih:([a-z\d]+)/i.exec(topic.magnet || '')?.[1]?.toUpperCase();
    const magnet = topic.magnet && !(replaceChanged && valid && current !== hash) ? topic.magnet
      : valid ? `magnet:?xt=urn:btih:${hash}&tr=${encodeURIComponent(TRACKER)}` : null;
    return {
      ...topic,
      seeders: row[at('seeders')] ?? topic.seeders,
      leechers: row[at('leechers')] ?? topic.leechers,
      size_bytes: row[at('tor_size_bytes')] ?? topic.size_bytes ?? null,
      registered_at: row[at('reg_time')] ? new Date(row[at('reg_time')] * 1000).toISOString() : topic.registered_at ?? null,
      peers_observed: observed,
      magnet,
      magnet_status: magnet ? 'available' : topic.magnet_status ?? 'not_found',
    };
  });
  return { catalog: { ...catalog, topics, api_observed: observed }, matched, unlisted: Object.keys(api.result).length - matched };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const platform = platformFrom(args);
  const index = args.indexOf('--catalog');
  const path = resolve(index >= 0 ? args[index + 1]
    : fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json`, import.meta.url)));
  const catalog = JSON.parse(await readFile(path, 'utf8'));
  const response = await fetch(`https://api.rutracker.cc/v1/static/pvc/f/${platform.forumId}`);
  if (!response.ok) throw new Error(`RuTracker API: HTTP ${response.status}`);
  const { catalog: updated, matched, unlisted } = applyApi(catalog, await response.json());
  const temporary = `${path}.${process.pid}.tmp`;
  await writeFile(temporary, JSON.stringify(updated, null, 2) + '\n');
  await rename(temporary, path);
  const magnets = updated.topics.filter(topic => topic.magnet).length;
  console.log(`${matched}/${updated.topics.length} topics found in the API; ${magnets} have magnets; ${unlisted} API topics aren't in the listing.`);
}

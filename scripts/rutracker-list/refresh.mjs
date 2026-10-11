#!/usr/bin/env node
// Bring a source (Russian) catalog up to date without a browser, for the daily catalog workflow:
// new and renamed releases from the forum's Atom feed (titles; it covers the last 1–4 weeks), and
// magnets, sizes and peers for every release from the public API. Release details from topic pages
// (genre, title ID, languages) need the browser collector; new topics arrive without them.
//
//   node refresh.mjs --platform ps5 [--catalog dist/rutracker/ps5-topics.json.ru-backup.json]
import { readFile, rename, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { applyApi } from './api.mjs';
import { platformFrom } from './platforms.mjs';

const decode = text => text.replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, '$1')
  .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(Number(n)))
  .replace(/&#x([\da-f]+);/gi, (_, n) => String.fromCodePoint(parseInt(n, 16)))
  .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');

/** The feed's releases: [{ id, title, author, updated }], newest first. */
export function parseFeed(xml) {
  return [...xml.matchAll(/<entry>([\s\S]*?)<\/entry>/g)].map(([, entry]) => {
    const pick = tag => decode(new RegExp(`<${tag}[^>]*>([\\s\\S]*?)</${tag}>`).exec(entry)?.[1] || '').trim();
    const link = /<link[^>]*href="([^"]+)"/.exec(entry)?.[1] || '';
    const id = /[?&]t=(\d+)/.exec(decode(link))?.[1];
    // "[Обновлено]" marks a re-uploaded release; it isn't part of the title.
    const title = pick('title').replace(/\s+/g, ' ').replace(/^\[Обновлено\]\s*/i, '');
    return { id, title, author: decode(/<name>([\s\S]*?)<\/name>/.exec(entry)?.[1] || '').trim() || null, updated: pick('updated') };
  }).filter(entry => entry.id && entry.title);
}

const sizeLabel = bytes => {
  if (!Number.isFinite(bytes) || bytes <= 0) return null;
  const [unit, scale] = bytes >= 2 ** 40 ? ['TB', 2 ** 40] : bytes >= 2 ** 30 ? ['GB', 2 ** 30] : ['MB', 2 ** 20];
  return `${(bytes / scale).toFixed(2)} ${unit}`;
};

/** "11.88 GB" → bytes (RuTracker's GB is 2^30), or null. */
export function labelBytes(label) {
  const match = /([\d.,]+)\s*(TB|GB|MB|KB)/i.exec(String(label || '').replace(/\u00a0/g, ' '));
  if (!match) return null;
  const power = { KB: 10, MB: 20, GB: 30, TB: 40 }[match[2].toUpperCase()];
  return Number(match[1].replace(',', '.')) * 2 ** power;
}

/**
 * Apply the feed and the API to a catalog. New topics need both (a title and a torrent); topics
 * the API no longer lists were deleted or moved, and leave — unless the API answered for less
 * than 80% of the catalog, which means a bad response rather than a mass deletion.
 */
export function refreshCatalog(catalog, feed, api, platform, now = new Date()) {
  const known = new Map(catalog.topics.map(topic => [topic.id, topic]));
  const fresh = feed.filter(entry => !platform.tag || entry.title.toUpperCase().startsWith(`[${platform.tag.toUpperCase()}]`));
  let added = 0, renamed = 0;
  for (const entry of fresh) {
    const topic = known.get(entry.id);
    if (topic) {
      if (topic.title !== entry.title) { known.set(entry.id, { ...topic, title: entry.title }); renamed++; }
    } else if (api.result[entry.id]) {
      known.set(entry.id, { id: entry.id, title: entry.title, url: `https://rutracker.net/forum/viewtopic.php?t=${entry.id}`,
        size: null, seeders: null, leechers: null, author: entry.author, magnet: null, magnet_status: 'not_checked',
        game_info: null, game_info_status: 'not_collected', added_at: entry.updated || now.toISOString() });
      added++;
    }
  }
  const listed = [...known.values()].filter(topic => api.result[topic.id]).length;
  const trusted = listed >= known.size * 0.8;
  const topics = [...known.values()].filter(topic => !trusted || api.result[topic.id]);
  const { catalog: updated } = applyApi({ ...catalog, topics }, api, { replaceChanged: true });
  // The API's byte count is authoritative: a label that no longer matches it (a re-upload of a
  // different size, or none yet) is replaced; one that matches keeps RuTracker's own wording.
  for (const topic of updated.topics) {
    const shown = labelBytes(topic.size);
    if (topic.size_bytes > 0 && !(shown && Math.abs(shown - topic.size_bytes) <= topic.size_bytes * 0.01)) {
      topic.size = sizeLabel(topic.size_bytes);
    }
  }
  return {
    catalog: { ...updated, topic_count: updated.topics.length, refreshed_at: now.toISOString() },
    added, renamed, removed: known.size - topics.length,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const platform = platformFrom(args);
  const index = args.indexOf('--catalog');
  const path = resolve(index >= 0 ? args[index + 1]
    : fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json.ru-backup.json`, import.meta.url)));
  const get = async (url, as) => {
    const response = await fetch(url, { signal: AbortSignal.timeout(60_000), headers: { 'User-Agent': 'ps5-launcher-catalog' } });
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return response[as]();
  };
  const catalog = JSON.parse(await readFile(path, 'utf8'));
  const feed = parseFeed(await get(`https://feed.rutracker.cc/atom/f/${platform.forumId}.atom`, 'text'));
  const api = await get(`https://api.rutracker.cc/v1/static/pvc/f/${platform.forumId}`, 'json');
  const { catalog: updated, added, renamed, removed } = refreshCatalog(catalog, feed, api, platform);
  const temporary = `${path}.${process.pid}.tmp`;
  await writeFile(temporary, JSON.stringify(updated, null, 2) + '\n');
  await rename(temporary, path);
  console.log(`${platform.label}: ${updated.topics.length} releases · ${added} new · ${renamed} renamed · ${removed} removed (feed: ${feed.length} entries)`);
}

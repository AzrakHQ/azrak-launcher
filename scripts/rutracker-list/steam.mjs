#!/usr/bin/env node
// Artwork, descriptions and Linux ratings for PC releases, from Steam's public store API (no key)
// and ProtonDB. Adds a `steam` block to each topic of a PC catalog: the matching Steam app, its
// description, genres, release date, screenshots, trailer, which artwork exists, whether it has a
// Linux version, and ProtonDB's tier (how well the Windows version runs through Proton). Topics
// without a match get `{ appid: null }` and are tried again after 30 days.
//
// For the console catalogs (ps5, ps4) it only fills a gap: releases without a title ID, which
// the launcher can't find PlayStation artwork for (new releases from the daily refresh, whose
// details aren't read). There only an exact name match counts, and ProtonDB isn't asked.
//
//   node steam.mjs --platform linux|wine|ps5|ps4 [--catalog path] [--limit 300] [--retry]
import { readFile, rename, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { platformFrom } from './platforms.mjs';

const ASSETS = 'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps';
const RETRY_DAYS = 30;
const sleep = ms => new Promise(done => setTimeout(done, ms));

/** The game's name from a release title (mirrors pcgame::clean_name in the launcher). */
export function cleanName(title) {
  let name = String(title).replace(/^(?:\s*\[[^\]]*\]\s*)+/, '').split(' [')[0].trim();
  const trailing = /\s*(?:\((?:v\.?\s*)?[\d.,_\-/ ]+[a-z]?\)|\(v\.?[^)]*\)|\((?:build|rev|update|alpha|beta|early access|in development|в разработке)[^)]*\)|\+\s*\d+\s*dlc\b.*|\+\s*(?:dlc|ost|soundtrack|bonus|update)s?\b.*)\s*$/i;
  for (let shorter = name.replace(trailing, '').trim(); shorter && shorter !== name; shorter = name.replace(trailing, '').trim()) name = shorter;
  const parts = name.split(' / ');
  if (parts.length === 2) {
    const latin = s => (s.match(/[a-z]/gi) || []).length;
    name = (latin(parts[1]) > latin(parts[0]) ? parts[1] : parts[0]).trim();
  }
  return name;
}

/** Comparable form of a name: lower case, letters and digits, roman numerals as digits. */
export function normalize(name) {
  const roman = { ii: '2', iii: '3', iv: '4', v: '5', vi: '6', vii: '7', viii: '8', ix: '9', x: '10' };
  // Apostrophes join their word ("Director's" is one word, not "director" + "s").
  return String(name).toLowerCase().replace(/[™®©]/g, '').replace(/['’]/g, '').replace(/&/g, ' and ')
    .split(/[^\p{L}\p{N}]+/u).filter(Boolean).map(w => roman[w] || w)
    .filter(w => !['the', 'a', 'an', 'edition', 'definitive', 'deluxe', 'complete', 'goty', 'remastered', 'enhanced', 'collection', 'directors', 'cut', 'game', 'of', 'year'].includes(w))
    .join(' ');
}

/** The search result that is this game, or null: same name, or almost (the release's edition
 * words aside); with `exact`, the same name only. Soundtracks, DLC and demos never match. */
export function bestMatch(name, items, { exact = false } = {}) {
  const want = normalize(name);
  if (!want) return null;
  const words = new Set(want.split(' '));
  let best = null;
  for (const item of items || []) {
    if (item.type !== 'app' || /\b(soundtrack|ost|dlc|demo|artbook|season pass|playtest)\b/i.test(item.name)) continue;
    const have = normalize(item.name);
    let score;
    if (have === want) score = 1;
    else {
      const other = new Set(have.split(' '));
      const common = [...words].filter(w => other.has(w)).length;
      score = common / Math.max(words.size, other.size);
    }
    if (score >= (exact ? 1 : 0.8) && (!best || score > best.score)) best = { score, item };
  }
  return best?.item ?? null;
}

/** The compact block kept in the catalog, from appdetails and the asset checks. */
export function steamBlock(appid, data, assets, protondb, now = new Date()) {
  const movie = (data.movies || []).find(m => m.highlight) || (data.movies || [])[0];
  const video = movie && (movie.mp4?.max || movie.webm?.max || movie.hls_h264 || movie.dash_h264) || null;
  return {
    appid,
    name: data.name,
    short: (data.short_description || '').replace(/\s+/g, ' ').trim().slice(0, 1200),
    genres: (data.genres || []).map(g => g.description).filter(Boolean),
    release: data.release_date?.date || null,
    developers: data.developers || [],
    publishers: data.publishers || [],
    linux: Boolean(data.platforms?.linux),
    metacritic: data.metacritic?.score ?? null,
    header: data.header_image || null,
    shots: (data.screenshots || []).slice(0, 8).map(s => s.path_full).filter(Boolean),
    video,
    assets,
    protondb,
    checked_at: now.toISOString(),
  };
}

async function get(url, as = 'json') {
  for (const wait of [0, 30_000, 120_000]) {
    if (wait) await sleep(wait);
    let response;
    try {
      response = await fetch(url, { signal: AbortSignal.timeout(30_000), headers: { 'User-Agent': 'ps5-launcher-catalog (github.com/MohamedAliRashad/ps5-launcher)' } });
    } catch { continue; }
    if (response.status === 429 || response.status >= 500) continue;
    if (!response.ok) return null;
    return as === 'json' ? response.json() : response;
  }
  throw new Error(`${url}: rate-limited or unavailable`);
}

async function exists(url) {
  try {
    const response = await fetch(url, { method: 'HEAD', signal: AbortSignal.timeout(20_000) });
    return response.ok;
  } catch { return false; }
}

/** ProtonDB's tier for an app ("platinum", "gold", …), null if unrated, undefined if unreachable. */
let protondbReachable = true;
async function protondb(appid) {
  if (!protondbReachable) return undefined;
  try {
    const response = await fetch(`https://www.protondb.com/api/v1/reports/summaries/${appid}.json`, { signal: AbortSignal.timeout(15_000) });
    if (response.status === 404) return null;
    if (!response.ok) return undefined;
    return (await response.json()).tier ?? null;
  } catch {
    protondbReachable = false; // don't wait on it for every game
    console.log('ProtonDB is unreachable from here; ratings skipped this run');
    return undefined;
  }
}

/** A release of several games (an anthology, a trilogy, "A + B"): no single Steam app is it. */
export function collection(title) {
  return /^\s*\[(?:антология|трилогия|дилогия|тетралогия|квадрилогия|пенталогия|сборник|коллекция|anthology|trilogy|dilogy|duology|tetralogy|quadrilogy|collection|compilation)\]/i.test(title)
    || cleanName(title).includes(' + ');
}

/** The name without an edition suffix: "SILENT HILL: Townfall - Deluxe Edition" → "SILENT HILL:
 * Townfall", "Atomic Heart (Ultimate Edition)" → "Atomic Heart". Steam's search finds nothing for
 * many such release names, and its app is the base game. */
export function baseName(name) {
  return String(name)
    .replace(/\s*\([^()]*\b(?:edition|cut|version)\)\s*$/i, '')
    .replace(/\s*[-–—:]\s*(?:[\p{L}\p{N}'’™®]+\s+){0,3}(?:edition|cut)\s*$/iu, '')
    .replace(/\s+(?:digital\s+)?(?:deluxe|ultimate|definitive|complete|gold|special|standard|premium)\s+edition\s*$/i, '')
    .trim();
}

/** A console release the launcher has no title ID for, so no PlayStation artwork. */
export function needsSteamArt(topic) {
  return !(topic.game_info?.title_id || /(?:PPSA|CUSA)\d{5}/i.test(topic.title));
}

/** Look up one release. Returns the steam block (appid null when Steam doesn't have it). */
export async function lookup(title, now = new Date(), { exact = false, ratings = true } = {}) {
  if (collection(title)) return { appid: null, reason: 'collection', checked_at: now.toISOString() };
  const name = cleanName(title);
  const search = async term => (await get(`https://store.steampowered.com/api/storesearch/?term=${encodeURIComponent(term)}&l=english&cc=US`))?.items;
  let item = bestMatch(name, await search(name), { exact });
  // Nothing for "Game: Deluxe Edition": the base game's name, matched just as strictly.
  const shorter = baseName(name);
  if (!item && shorter && shorter !== name) {
    await sleep(1600);
    item = bestMatch(shorter, await search(shorter), { exact });
  }
  if (!item) return { appid: null, checked_at: now.toISOString() };
  await sleep(1600); // appdetails allows about 200 requests per 5 minutes
  const details = await get(`https://store.steampowered.com/api/appdetails?appids=${item.id}&l=english&cc=US`);
  const data = details?.[item.id]?.success ? details[item.id].data : null;
  if (!data) return { appid: null, checked_at: now.toISOString() };
  const base = `${ASSETS}/${item.id}`;
  const assets = {};
  for (const [key, file] of [['portrait', 'library_600x900_2x.jpg'], ['hero', 'library_hero.jpg'], ['logo', 'logo.png']]) {
    assets[key] = await exists(`${base}/${file}`) ? `${base}/${file}` : null;
  }
  return steamBlock(item.id, data, assets, ratings ? await protondb(item.id) : null, now);
}

const due = (topic, now, retry = false) => !topic.steam || (topic.steam.appid == null
  && (retry || now - new Date(topic.steam.checked_at || 0) > RETRY_DAYS * 86400_000));

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const platform = platformFrom(args);
  const console_ = Boolean(platform.tag);
  const option = (flag, fallback) => { const i = args.indexOf(flag); return i >= 0 ? args[i + 1] : fallback; };
  const path = resolve(option('--catalog', fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json.ru-backup.json`, import.meta.url))));
  const limit = Number(option('--limit', '100000'));
  const catalog = JSON.parse(await readFile(path, 'utf8'));
  const now = new Date();
  // --retry: look up unmatched releases again now (after the matching improved).
  const retry = args.includes('--retry');
  const todo = catalog.topics.filter(t => due(t, now, retry) && (!console_ || needsSteamArt(t))).slice(0, limit);
  console.log(`${platform.label}: ${todo.length} topics to look up`);
  let found = 0, done = 0;
  const save = async () => {
    const temporary = `${path}.${process.pid}.tmp`;
    await writeFile(temporary, JSON.stringify(catalog, null, 2) + '\n');
    await rename(temporary, path);
  };
  for (const topic of todo) {
    try {
      topic.steam = await lookup(topic.title, now, console_ ? { exact: true, ratings: false } : {});
    } catch (error) {
      console.log(`Stopping: ${error.message}`);
      break;
    }
    if (topic.steam.appid) found++;
    if (++done % 25 === 0) { await save(); console.log(`${done}/${todo.length} looked up, ${found} on Steam`); }
    await sleep(1600);
  }
  await save();
  console.log(`${platform.label}: ${done} looked up, ${found} found on Steam`);
}

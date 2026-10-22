#!/usr/bin/env node
// Artwork, descriptions and Linux ratings for PC releases, from Steam's public store API (no key)
// and ProtonDB. Adds a `steam` block to each topic of a PC catalog: the matching Steam app, its
// description, genres, release date, screenshots, trailer, which artwork exists, whether it has a
// Linux version, and ProtonDB's tier (how well the Windows version runs through Proton). Topics
// without a match get `{ appid: null }` and are tried again after 30 days; a match whose artwork or
// rating couldn't be checked (network, rate limit) is marked `partial` and looked up again next run.
//
// For the console catalogs (ps5, ps4) it only fills a gap: releases without a title ID, which
// the launcher can't find PlayStation artwork for (new releases from the daily refresh, whose
// details aren't read). There only an exact name match counts, and ProtonDB isn't asked.
//
//   node steam.mjs --platform linux|wine|ps5|ps4 [--catalog path] [--limit 300] [--minutes 40] [--retry]
import { readFile, rename, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { platformFrom } from './platforms.mjs';

const STORE = 'https://shared.akamai.steamstatic.com/store_item_assets';
const ASSETS = `${STORE}/steam/apps`;
const RETRY_DAYS = 30;
const sleep = ms => new Promise(done => setTimeout(done, ms));

/** Words that only describe a version: "(v.build 9218157)", "(pre-Alpha 1.7.0c)",
 * "(часть 1 и 2, версия 1.07)", "(Early Access)". */
const VERSION_WORDS = new Set(['v', 'v.', 'build', 'rev', 'revision', 'release', 'patch', 'hotfix', 'update', 'upd',
  'alpha', 'beta', 'pre-alpha', 'pre-beta', 'rc', 'indev', 'milestone', 'early', 'access', 'in', 'development', 'версия',
  'сборка', 'билд', 'патч', 'обновление', 'часть', 'и', 'в', 'разработке', '-', '–', '—', '/']);
const versionToken = token => VERSION_WORDS.has(token)
  || /^[a-z]{0,3}\d[\w.\-/:]*$/.test(token)                 // 1.6.0f2, v1.228, 30.06.22, 2026, b18, rv22189, pr1.8347
  || /^(?:build|indev|alpha|beta|pre-alpha)[-.]?\d[\w.\-]*$/.test(token) // indev-1588
  || /^[0-9a-f]{7,40}$/.test(token);                           // a build's commit: a0cef1aaa
// "+ 3 DLC", "+ OST", "+ All DLC", "+ 2 Soundtrack Albums!", "+ Mods", "+ Dedicated Server".
const EXTRAS = /\s*\+\s*(?:\d+\s*)?(?:all\s+)?(?:dlcs?|ost|soundtracks?|bonus(?:es)?|updates?|mods?|dedicated server)\b[^()]*$/i;

/** Whether a bracketed group only gives a version, build, date or year (the brackets inside it and
 * any "+ DLC" aside). */
export function versionGroup(content) {
  // An add-on: "(+ DLC)", "(+DLC Goatperson)", "(+ Space Age)".
  if (content.trim().startsWith('+')) return true;
  const text = content.replace(/[()]/g, ' ').replace(/\s*\+\s*(?:\d+\s*)?(?:all\s+)?(?:dlcs?|ost|soundtracks?|bonus(?:es)?|updates?|mods?)\b/gi, ' ')
    .toLowerCase().replace(/\bv\.(?=\S)/g, 'v. ');
  const tokens = text.split(/[\s,;]+/).filter(Boolean);
  // "+" between versions ("1.29.6 + 1.35.5"), not between games ("1 + 2").
  const versions = tokens.some(token => /\d\.\d/.test(token));
  return tokens.every(token => versionToken(token) || (token === '+' && versions));
}

/** The game's name from a release title (mirrors pcgame::clean_name in the launcher; both are
 * tested against clean-names.json). */
export function cleanName(title) {
  let name = String(title).replace(/^(?:\s*\[[^\]]*\]\s*)+/, '').split(' [')[0].trim();
  // "Russian / English" titles: the part with Latin letters (a " / " inside brackets doesn't count).
  const split = topLevel(name, ' / ');
  if (split.length === 2) {
    const latin = s => (s.match(/[a-z]/gi) || []).length;
    name = (latin(split[1]) > latin(split[0]) ? split[1] : split[0]).trim();
  }
  // Bracketed versions, builds, dates and years, wherever they are (nested brackets included).
  let out = '';
  for (let i = 0; i < name.length; i++) {
    if (name[i] !== '(') { out += name[i]; continue; }
    let depth = 0, j = i;
    for (; j < name.length; j++) {
      if (name[j] === '(') depth++;
      else if (name[j] === ')' && --depth === 0) break;
    }
    const group = name.slice(i, j + 1);
    // A removed group takes the space before it: "Age of Empires II (2): …" → "Age of Empires II: …".
    if (versionGroup(group.slice(1, j < name.length ? -1 : undefined))) out = out.trimEnd();
    else out += group;
    i = j;
  }
  name = out.replace(/\s+/g, ' ').trim();
  for (let shorter = name.replace(EXTRAS, '').trim(); shorter && shorter !== name; shorter = name.replace(EXTRAS, '').trim()) name = shorter;
  // A bare version at the end: "DROD: The City Beneath v3.3.1.98".
  return name.replace(/\s+v\.?\d[\w.\-]*$/i, '').trim() || name;
}

/** `text` split at `separator` where it isn't inside brackets. */
function topLevel(text, separator) {
  const parts = [];
  let depth = 0, start = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '(') depth++;
    else if (text[i] === ')') depth = Math.max(0, depth - 1);
    else if (depth === 0 && text.startsWith(separator, i)) { parts.push(text.slice(start, i)); start = i + separator.length; i += separator.length - 1; }
  }
  parts.push(text.slice(start));
  return parts;
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
export function steamBlock(appid, data, assets, protondb, now = new Date(), partial = false) {
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
    ...(partial ? { partial: true } : {}),
  };
}

async function get(url, as = 'json') {
  for (const wait of [0, 30_000, 120_000]) {
    if (wait) await sleep(wait);
    let response;
    try {
      response = await fetch(url, { signal: AbortSignal.timeout(30_000), headers: { 'User-Agent': 'azrak-catalog (github.com/AzrakHQ/azrak-launcher)' } });
    } catch { continue; }
    if (response.status === 429 || response.status >= 500) continue;
    if (!response.ok) return null;
    return as === 'json' ? response.json() : response;
  }
  throw new Error(`${url}: rate-limited or unavailable`);
}

/** Whether the file is there: true, false, or undefined when that couldn't be told (network,
 * rate limit, server error). */
async function exists(url) {
  try {
    const response = await fetch(url, { method: 'HEAD', signal: AbortSignal.timeout(20_000) });
    if (response.status === 429 || response.status >= 500) return undefined;
    return response.ok;
  } catch { return undefined; }
}

/** The artwork Steam lists for these apps, as { appid: { portrait, hero } } (null where an app has
 * none; apps Steam didn't answer for are left out), or undefined when Steam couldn't be asked.
 * Apps from 2024 on keep their files under hashed paths, which a guessed `library_600x900_2x.jpg`
 * misses: only Steam's own list finds them. */
export async function storeAssets(appids) {
  const input = { ids: appids.map(appid => ({ appid })), context: { language: 'english', country_code: 'US' },
    data_request: { include_assets: true } };
  let body;
  try {
    const response = await fetch(`https://api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json=${encodeURIComponent(JSON.stringify(input))}`,
      { signal: AbortSignal.timeout(30_000) });
    if (!response.ok) return undefined;
    body = await response.json();
  } catch { return undefined; }
  const found = {};
  for (const item of body?.response?.store_items || []) {
    if (item.success !== 1) continue;
    const assets = item.assets || {};
    const url = file => file && assets.asset_url_format ? `${STORE}/${assets.asset_url_format.replace('${FILENAME}', file)}` : null;
    // DLC (a PS5 release can be one on Steam) has no library portrait: its near-portrait capsule.
    const portrait = assets.library_capsule_2x || assets.library_capsule || assets.hero_capsule_2x || assets.hero_capsule;
    found[item.id] = { portrait: url(portrait), hero: url(assets.library_hero) };
  }
  return found;
}

/** Matches without a portrait or hero (from before Steam's own artwork list was read, or that had
 * none then) take them from that list, 50 apps a request. Returns how many gained artwork. */
export async function fillAssets(topics) {
  const missing = topics.filter(topic => topic.steam?.appid && (!topic.steam.assets?.portrait || !topic.steam.assets?.hero));
  const ids = [...new Set(missing.map(topic => topic.steam.appid))];
  const listed = {};
  for (let i = 0; i < ids.length; i += 50) {
    if (i) await sleep(1000);
    const batch = await storeAssets(ids.slice(i, i + 50));
    if (!batch) break;
    Object.assign(listed, batch);
  }
  let filled = 0;
  for (const topic of missing) {
    const art = listed[topic.steam.appid];
    const had = topic.steam.assets || {};
    if (!art || (had.portrait || !art.portrait) && (had.hero || !art.hero)) continue;
    topic.steam = { ...topic.steam, assets: { ...had, portrait: had.portrait || art.portrait, hero: had.hero || art.hero } };
    filled++;
  }
  return filled;
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

/** Look up one release. Returns the steam block (appid null when Steam doesn't have it; `query`
 * keeps the name searched for, so a better cleanName searches again). */
export async function lookup(title, now = new Date(), { exact = false, ratings = true } = {}) {
  const name = cleanName(title);
  if (collection(title)) return { appid: null, reason: 'collection', query: name, checked_at: now.toISOString() };
  const search = async term => (await get(`https://store.steampowered.com/api/storesearch/?term=${encodeURIComponent(term)}&l=english&cc=US`))?.items;
  let item = bestMatch(name, await search(name), { exact });
  // Nothing for "Game: Deluxe Edition": the base game's name, matched just as strictly.
  const shorter = baseName(name);
  if (!item && shorter && shorter !== name) {
    await sleep(1600);
    item = bestMatch(shorter, await search(shorter), { exact });
  }
  if (!item) return { appid: null, query: name, checked_at: now.toISOString() };
  await sleep(1600); // appdetails allows about 200 requests per 5 minutes
  const details = await get(`https://store.steampowered.com/api/appdetails?appids=${item.id}&l=english&cc=US`);
  const data = details?.[item.id]?.success ? details[item.id].data : null;
  if (!data) return { appid: null, query: name, checked_at: now.toISOString() };
  const listed = (await storeAssets([item.id]))?.[item.id];
  // Steam's list has no logo: it's where older apps keep theirs, or none.
  const logo = `${ASSETS}/${item.id}/logo.png`;
  const hasLogo = await exists(logo);
  const assets = { portrait: listed?.portrait ?? null, hero: listed?.hero ?? null, logo: hasLogo ? logo : null };
  let partial = !listed || hasLogo === undefined;
  const rating = ratings ? await protondb(item.id) : null;
  if (rating === undefined) partial = true;
  return steamBlock(item.id, data, assets, rating ?? null, now, partial);
}

/** Whether a topic needs (another) lookup: never looked up; a match whose artwork or rating
 * couldn't be checked (`partial`, or no ProtonDB answer from before that flag); no match for 30
 * days, or searched under a name cleanName now gives differently. With `retry`: every miss, and
 * matches missing artwork. */
export function due(topic, now, retry = false) {
  const steam = topic.steam;
  if (!steam) return true;
  if (steam.appid == null) return retry || steam.query !== cleanName(topic.title)
    || now - new Date(steam.checked_at || 0) > RETRY_DAYS * 86400_000;
  return Boolean(steam.partial) || !('protondb' in steam)
    || (retry && Object.values(steam.assets || {}).some(url => url == null));
}

/** Look a topic up (again), or for a match that only lacks its ProtonDB rating (ProtonDB was
 * unreachable then), just ask ProtonDB: a full lookup takes seconds, the rating a fraction. */
export async function update(topic, now = new Date(), { console_ = false } = {}) {
  const steam = topic.steam;
  if (!console_ && steam?.appid && !steam.partial && !('protondb' in steam)) {
    const rating = await protondb(steam.appid);
    return rating === undefined ? steam : { ...steam, protondb: rating };
  }
  return lookup(topic.title, now, console_ ? { exact: true, ratings: false } : {});
}

/** Releases never looked up first, then the oldest checks: lookups that keep failing (a
 * `partial` match whose artwork server keeps erroring) must not take every day's slots. */
export function lookupOrder(topics) {
  const checked = t => t.steam ? new Date(t.steam.checked_at || 0).getTime() : -Infinity;
  return [...topics].sort((a, b) => checked(a) - checked(b));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const platform = platformFrom(args);
  const console_ = Boolean(platform.tag);
  const option = (flag, fallback) => { const i = args.indexOf(flag); return i >= 0 ? args[i + 1] : fallback; };
  const path = resolve(option('--catalog', fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json.ru-backup.json`, import.meta.url))));
  const limit = Number(option('--limit', '100000'));
  // --minutes: stop looking up after this long (the rest follow on later runs).
  const until = Date.now() + Number(option('--minutes', '600')) * 60_000;
  const catalog = JSON.parse(await readFile(path, 'utf8'));
  const now = new Date();
  // --retry: look up unmatched releases again now (after the matching improved).
  const retry = args.includes('--retry');
  // Artwork first: one request covers 50 matches, where a lookup takes seconds.
  const filled = await fillAssets(catalog.topics);
  if (filled) console.log(`${platform.label}: artwork found for ${filled} earlier matches`);
  const todo = lookupOrder(catalog.topics.filter(t => due(t, now, retry) && (!console_ || needsSteamArt(t)))).slice(0, limit);
  console.log(`${platform.label}: ${todo.length} topics to look up`);
  let found = 0, done = 0;
  const save = async () => {
    const temporary = `${path}.${process.pid}.tmp`;
    await writeFile(temporary, JSON.stringify(catalog, null, 2) + '\n');
    await rename(temporary, path);
  };
  for (const topic of todo) {
    if (Date.now() > until) { console.log('Time budget used; the rest follow on the next run'); break; }
    const ratingOnly = topic.steam?.appid && !('protondb' in topic.steam);
    try {
      topic.steam = await update(topic, now, { console_ });
    } catch (error) {
      console.log(`Stopping: ${error.message}`);
      break;
    }
    if (topic.steam.appid) found++;
    if (++done % 25 === 0) { await save(); console.log(`${done}/${todo.length} looked up, ${found} on Steam`); }
    await sleep(ratingOnly ? 300 : 1600);
  }
  await save();
  console.log(`${platform.label}: ${done} looked up, ${found} found on Steam`);
}

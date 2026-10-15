#!/usr/bin/env node
import assert from 'node:assert/strict';
import { access, copyFile, readFile, rename, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { englishCatalog, hasRussian, isProtected, keepsNumbers, prepareCatalog, protectTechnical, translationInputs, transliterate, latinLookalikes } from './english.mjs';
import { platformFrom } from './platforms.mjs';

const platform = platformFrom(process.argv.slice(2));
const output = fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json`, import.meta.url));
const cachePath = fileURLToPath(new URL('../../dist/rutracker/english-translations.json', import.meta.url));
const original = JSON.parse(await readFile(process.argv.includes('--from-original') ? `${output}.ru-backup.json` : output, 'utf8'));
// Steam data (PC catalogs) is in English already and may hold names in other scripts: it skips
// translation and is put back afterwards.
const steam = new Map(original.topics.filter(t => t.steam).map(t => [t.id, t.steam]));
const prepared = prepareCatalog({ ...original, topics: original.topics.map(({ steam: _, ...t }) => t) });
const inputs = translationInputs(prepared);
let cache = {};
try { cache = JSON.parse(await readFile(cachePath, 'utf8')); } catch (error) { if (error.code !== 'ENOENT') throw error; }
// Repair older cached translations that reformatted version numbers or dates.
for (const text of inputs) {
  if (cache[text] && !keepsNumbers(text, cache[text])) delete cache[text];
}
const transliterated = [];
const pending = inputs.filter(text => !cache[text] || hasRussian(cache[text]));
const total = pending.length;
console.log(`${inputs.length} unique strings require translation; ${pending.length} not cached.`);

// Google's translator as translate.googleapis.com serves it (`gtx`), or, once that rate-limits
// this address (it does for some, GitHub's runners among them), as Chrome's translate extension
// reaches it: the same translations, under separate limits. Each returns one line per line asked.
async function gtx(values) {
  const url = new URL('https://translate.googleapis.com/translate_a/single');
  url.search = new URLSearchParams({ client: 'gtx', sl: 'ru', tl: 'en', dt: 't', q: values.join('\n') });
  const response = await fetch(url, { signal: AbortSignal.timeout(30_000) });
  if (!response.ok) return { ok: false, status: response.status };
  const data = await response.json();
  return { ok: true, lines: data[0].map(segment => segment[0] || '').join('').split(/\r?\n/).map(line => line.trim()) };
}
async function chrome(values) {
  const body = new URLSearchParams();
  for (const value of values) body.append('q', value);
  const response = await fetch('https://clients5.google.com/translate_a/t?client=dict-chrome-ex&sl=ru&tl=en',
    { method: 'POST', body, signal: AbortSignal.timeout(30_000) });
  if (!response.ok) return { ok: false, status: response.status };
  // A translation per value (or [translation, language] pairs).
  return { ok: true, lines: (await response.json()).map(item => String(Array.isArray(item) ? item[0] : item).trim()) };
}
let endpoint = gtx;

async function translateLines(lines) {
  const protectedLines = lines.map(protectTechnical);
  const values = protectedLines.map(line => line.value);
  // The public endpoints rate-limit (429) and have bad moments (5xx): wait and try again.
  let response;
  for (const wait of [0, 20_000, 60_000, 180_000]) {
    if (wait) {
      console.log(`Translation endpoint returned HTTP ${response.status}; retrying in ${wait / 1000} s`);
      await new Promise(done => setTimeout(done, wait));
    }
    for (;;) {
      try {
        response = await endpoint(values);
      } catch (error) {
        // A network failure is retried like a rate limit.
        response = { ok: false, status: `unreachable (${error.cause?.code || error.name})` };
      }
      if (response.ok || response.status !== 429 || endpoint === chrome) break;
      console.log('Translation endpoint rate-limits this address (HTTP 429); using the Chrome extension endpoint');
      endpoint = chrome;
    }
    if (response.ok || !(response.status === 429 || response.status >= 500 || String(response.status).startsWith('unreachable'))) break;
  }
  if (!response.ok) throw new Error(`Translation endpoint returned HTTP ${response.status}; source catalog unchanged`);
  if (response.lines.length !== lines.length) return [];
  // A protected number the translator mangled: none of the batch counts (it's retried a line at a
  // time; a line still mangled is spelled in Latin letters instead).
  try {
    return response.lines.map((line, index) => protectedLines[index].restore(line));
  } catch {
    return [];
  }
}
let done = 0;
while (pending.length) {
  const batch = [];
  let length = 0;
  while (pending.length && batch.length < 16 && length + pending[0].length < 2200) {
    const text = pending.shift(); batch.push(text); length += text.length + 1;
  }
  if (!batch.length) batch.push(pending.shift());
  let translations = await translateLines(batch);
  if (translations.length !== batch.length) {
    translations = [];
    for (const text of batch) {
      translations.push((await translateLines([text])).join(' '));
      await new Promise(resolve => setTimeout(resolve, 300));
    }
  }
  for (let index = 0; index < batch.length; index++) {
    let value = translations[index];
    if (!value || hasRussian(value)) {
      // The translator returned it unchanged (a code, typo or name): spell it in Latin letters.
      value = transliterate(value || batch[index]);
      transliterated.push(batch[index]);
      if (!value || hasRussian(value)) throw new Error(`Translation incomplete: ${batch[index]}; source catalog unchanged`);
    }
    assert.ok(keepsNumbers(batch[index], value), `Numeric/version/date tokens changed: ${batch[index]} → ${value}`);
    cache[batch[index]] = value.replace(/\bsoftware (?=\d+(?:\.|x))/gi, 'firmware ')
      .replace(/\bChinese \(traditional\)/gi, 'Chinese (Traditional)')
      .replace(/\bChinese \(simplified\)/gi, 'Chinese (Simplified)')
      .replace(/(\d{1,2}\.\d{1,2}\.\d{2,4})\s*g\.?\b/gi, '$1');
  }
  await writeFile(`${cachePath}.tmp`, JSON.stringify(cache, null, 2) + '\n');
  await rename(`${cachePath}.tmp`, cachePath);
  done += batch.length;
  console.log(`Translated ${done}/${total} strings; ${pending.length} remaining`);
  if (pending.length) await new Promise(resolve => setTimeout(resolve, 400));
}

if (transliterated.length) console.log(`Transliterated ${transliterated.length} strings the translator left in Russian: ${transliterated.join(' | ')}`);
const result = englishCatalog(prepared, cache);
assert.equal(result.topics.length, original.topics.length);
for (let index = 0; index < original.topics.length; index++) {
  const before = original.topics[index], after = result.topics[index];
  for (const key of ['id', 'url', 'magnet', 'author', 'size', 'seeders', 'leechers']) assert.deepEqual(after[key], before[key], key);
  // New releases from the daily refresh have no details until the browser collector reads them.
  if (!before.game_info) { assert.equal(after.game_info, before.game_info); continue; }
  // Title IDs are compared after look-alike normalization (prepareCatalog), the only change allowed.
  for (const key of ['source_url', 'image_urls']) assert.deepEqual(after.game_info[key], before.game_info[key], key);
  assert.deepEqual(after.game_info.title_id, latinLookalikes(before.game_info.title_id), 'title_id');
}
function validate(value, key = '') {
  if (typeof value === 'string' && !isProtected(key, value)) assert.equal(hasRussian(value), false, key);
  else if (Array.isArray(value)) value.forEach(item => validate(item, key));
  else if (value && typeof value === 'object') for (const [name, item] of Object.entries(value)) {
    assert.equal(hasRussian(name), false, name); validate(item, name);
  }
}
validate(result);
for (const topic of result.topics) if (steam.has(topic.id)) topic.steam = steam.get(topic.id);
const backup = `${output}.ru-backup.json`;
try { await access(backup); } catch (error) { if (error.code !== 'ENOENT') throw error; await copyFile(output, backup); }
result.translation = { language: 'en', source_language: 'ru', translated_at: new Date().toISOString(),
  method: 'Reviewed terminology glossary and Google Translate public metadata translation',
  protected_data: 'IDs, URLs, magnets, usernames, image URLs and peer statistics preserved' };
await writeFile(`${output}.tmp`, JSON.stringify(result, null, 2) + '\n');
await rename(`${output}.tmp`, output);
console.log(`English catalog saved: ${result.topics.length} topics. Original preserved in ${backup}`);
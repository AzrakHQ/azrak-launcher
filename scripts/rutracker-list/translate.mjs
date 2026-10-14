#!/usr/bin/env node
import assert from 'node:assert/strict';
import { access, copyFile, readFile, rename, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { englishCatalog, hasRussian, isProtected, prepareCatalog, protectTechnical, translationInputs, transliterate, latinLookalikes } from './english.mjs';
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
const numericSignature = text => text.match(/\d+(?:[.,/]\d+|\.x+|\.\?+)*/gi) || [];
let cache = {};
try { cache = JSON.parse(await readFile(cachePath, 'utf8')); } catch (error) { if (error.code !== 'ENOENT') throw error; }
// Repair older cached translations that reformatted version numbers or dates.
for (const text of inputs) {
  if (cache[text] && JSON.stringify(numericSignature(text)) !== JSON.stringify(numericSignature(cache[text]))) delete cache[text];
}
const transliterated = [];
const pending = inputs.filter(text => !cache[text] || hasRussian(cache[text]));
const total = pending.length;
console.log(`${inputs.length} unique strings require translation; ${pending.length} not cached.`);

async function translateLines(lines) {
  const protectedLines = lines.map(protectTechnical);
  const url = new URL('https://translate.googleapis.com/translate_a/single');
  url.search = new URLSearchParams({ client: 'gtx', sl: 'ru', tl: 'en', dt: 't', q: protectedLines.map(line => line.value).join('\n') });
  // The public endpoint rate-limits (429) and has bad moments (5xx): wait and try again.
  let response;
  for (const wait of [0, 20_000, 60_000, 180_000]) {
    if (wait) {
      console.log(`Translation endpoint returned HTTP ${response.status}; retrying in ${wait / 1000} s`);
      await new Promise(done => setTimeout(done, wait));
    }
    try {
      response = await fetch(url, { signal: AbortSignal.timeout(30_000) });
    } catch (error) {
      // A network failure is retried like a rate limit.
      response = { ok: false, status: `unreachable (${error.cause?.code || error.name})` };
      continue;
    }
    if (response.ok || !(response.status === 429 || response.status >= 500)) break;
  }
  if (!response.ok) throw new Error(`Translation endpoint returned HTTP ${response.status}; source catalog unchanged`);
  const data = await response.json();
  const text = data[0].map(segment => segment[0] || '').join('');
  const translated = text.split(/\r?\n/).map(line => line.trim());
  if (translated.length !== lines.length) return [];
  return translated.map((line, index) => protectedLines[index].restore(line));
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
    assert.deepEqual(numericSignature(value), numericSignature(batch[index]), 'Numeric/version/date tokens changed');
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
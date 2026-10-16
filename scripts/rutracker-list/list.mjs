#!/usr/bin/env node
import { chromium } from 'playwright-core';
import { spawn, execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile, rename } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readListing, mergeTopics } from './listing.mjs';
import { collectMagnetBatch, applyMagnets } from './magnets.mjs';
import { platformFrom } from './platforms.mjs';

const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  if (index < 0) return fallback;
  if (!args[index + 1] || args[index + 1].startsWith('--')) throw new Error(`Missing value for ${name}`);
  return args[index + 1];
}
if (args.includes('--help')) {
  console.log('Usage: node list.mjs [--platform ps5|ps4|linux|wine] [--output path] [--max-pages 200] [--verification-timeout 180] [--headless] [--include-magnets] [--enrich-existing path] [--concurrency 1-4]\n'
    + 'Reads listing/topic HTML only. Complete verification/login manually. Never opens magnet links or downloads torrents.');
  process.exit(0);
}
const platform = platformFrom(args);
const output = resolve(option('--output', fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json`, import.meta.url))));
const maxPages = Number(option('--max-pages', '200'));
const concurrency = Number(option('--concurrency', '1'));
const headless = args.includes('--headless');
const existingPath = option('--enrich-existing', null);
const includeMagnets = args.includes('--include-magnets') || existingPath !== null;
const existing = existingPath ? JSON.parse(await readFile(resolve(existingPath), 'utf8')) : null;
if (existing && (!Array.isArray(existing.topics) || existing.topics.some(topic => !/^\d+$/.test(topic.id)))) {
  throw new Error('Existing catalog must have a topics array containing numeric topic IDs');
}
const verificationSeconds = Number(option('--verification-timeout', headless ? '0' : '180'));
if (!Number.isSafeInteger(maxPages) || maxPages < 1 || maxPages > 200
    || !Number.isFinite(verificationSeconds) || verificationSeconds < 0 || verificationSeconds > 600) {
  throw new Error('max-pages must be 1–200 and verification-timeout must be 0–600 seconds');
}
const profile = resolve(homedir(), '.cache/azrak/rutracker-list-browser');
const pages = [];
const pending = new Set([platform.source]);
const visited = new Set();
const failures = [];
let context;
let browser;
let chrome;
let enriched;
let interrupted = false;
const interrupt = () => { interrupted = true; void context?.close(); chrome?.kill(); };
process.once('SIGINT', interrupt);
process.once('SIGTERM', interrupt);

try {
  console.log('Opening a dedicated Chrome profile. If requested, complete verification/login yourself.');
  console.log(includeMagnets
    ? 'Reading forum/topic HTML and magnet strings only; no magnet links are opened and no files are downloaded.'
    : 'Only forum listings will be collected; no topics, magnets, torrents or game files are fetched.');
  if (headless) {
    context = await chromium.launchPersistentContext(profile, {
      channel: 'chrome', headless, acceptDownloads: false, serviceWorkers: 'block',
      viewport: { width: 1280, height: 800 },
    });
  } else {
    // Cloudflare's check keeps looping in a browser that automation launched (automation flag,
    // navigator.webdriver). Start the user's own Chrome normally, with this dedicated profile,
    // and only attach to it afterwards.
    ({ browser, chrome } = await startChrome(profile));
    context = browser.contexts()[0];
  }
  await context.route('**/*', route => {
    const request = route.request();
    const url = new URL(request.url());
    if (!['https:', 'http:'].includes(url.protocol)
        || /\/(?:dl|download)\.php$/.test(url.pathname)
        || /\.(?:torrent|rar|7z|zip|pkg)$/i.test(url.pathname)) {
      return route.abort();
    }
    if (request.isNavigationRequest() && request.frame() === request.frame().page().mainFrame()
        && url.hostname !== 'rutracker.net') return route.abort();
    return route.continue();
  });
  const page = context.pages()[0] || await context.newPage();
  while (pending.size && visited.size < maxPages && !interrupted) {
    const url = pending.values().next().value;
    pending.delete(url);
    if (visited.has(url)) continue;
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 45_000 });
    let result = await page.evaluate(readListing, platform);
    if (result.status !== 'ok' && verificationSeconds > 0) {
      console.log(`Waiting up to ${verificationSeconds}s for manual verification/login at ${url}`);
      try {
        await page.waitForFunction(() => !/just a moment|performing security verification/i.test(document.title)
          && document.querySelector('a.torTopic[href], a.topictitle[href]') !== null,
        null, { polling: 1000, timeout: verificationSeconds * 1000 });
      } catch { /* Report the actual page rather than interpreting a challenge as an empty result. */ }
      result = await page.evaluate(readListing, platform);
    }
    if (result.status !== 'ok') {
      failures.push({ url, status: result.status });
      break;
    }
    visited.add(url);
    if (existing) { pending.clear(); break; }
    pages.push(result);
    for (const next of result.pageUrls) if (!visited.has(next)) pending.add(next);
    console.log(`Page ${visited.size}: ${result.topics.length} ${platform.label} topics; ${pending.size} pages remaining`);
    // Sequential, polite requests. This is rate limiting, not challenge solving.
    if (pending.size) await new Promise(done => setTimeout(done, 1500));
  }
  if (includeMagnets && !interrupted && failures.length === 0 && pending.size === 0) {
    const base = existing || { topics: mergeTopics(pages) };
    const ids = base.topics.filter(topic => topic.magnet_status !== 'available' || topic.game_info_status !== 'available').map(topic => topic.id);
    const batches = [];
    for (let offset = 0; offset < ids.length && !interrupted; offset += 20) {
      const batch = await page.evaluate(collectMagnetBatch, { topicIds: ids.slice(offset, offset + 20), concurrency });
      batches.push(batch);
      enriched = applyMagnets(base, batches);
      console.log(`Magnets: ${enriched.magnet_collection.available}/${base.topics.length}; ${enriched.magnet_collection.remaining} remaining`);
      if (!batch.complete) break;
      if (offset + 20 < ids.length) await new Promise(done => setTimeout(done, 1500));
    }
    enriched ??= applyMagnets(base, batches);
  }
} catch (error) {
  failures.push({ status: interrupted ? 'cancelled' : 'browser_error', message: error.message });
} finally {
  await (browser ? browser.close() : context?.close())?.catch(() => {});
  chrome?.kill();
}

const topics = mergeTopics(pages);
const complete = !interrupted && failures.length === 0 && pending.size === 0;
let report = existing ? { ...existing } : {
  source: platform.source,
  platform: platform.key,
  collected_at: new Date().toISOString(),
  complete,
  scope: `${platform.label} release topic listings in forum ${platform.forumId}; not a count of distinct games or an atomic site-wide snapshot`,
  pages_visited: [...visited],
  pages_remaining: [...pending],
  failures,
  topic_count: topics.length,
  topics,
};
if (includeMagnets) {
  const magnetData = enriched || applyMagnets(report, []);
  report = { ...report, topics: magnetData.topics, magnet_collection: magnetData.magnet_collection,
    game_info_collection: magnetData.game_info_collection };
}
await mkdir(dirname(output), { recursive: true });
const temporary = `${output}.${process.pid}.tmp`;
await writeFile(temporary, JSON.stringify(report, null, 2) + '\n');
const successful = complete && (!includeMagnets || (report.magnet_collection.complete && report.game_info_collection.complete));
if (successful) {
  await rename(temporary, output);
} else {
  // Never replace a previously complete catalog with partial/blocked results.
  await rename(temporary, `${output}.partial.json`);
}
console.log(`${successful ? 'Complete' : 'Partial/blocked'}: ${report.topic_count} topics; ${visited.size} listing pages visited in this run.`);
console.log(`Saved ${successful ? output : `${output}.partial.json`}`);
process.exitCode = successful ? 0 : 2;
/** Launch Chrome as a normal process with a debugging port, then attach Playwright to it. */
async function startChrome(profileDir) {
  const binary = ['google-chrome-stable', 'google-chrome', 'chromium', 'chromium-browser'].find(name => {
    try { execFileSync('which', [name], { stdio: 'ignore' }); return true; } catch { return false; }
  });
  if (!binary) throw new Error('Google Chrome or Chromium is required');
  const port = 20000 + Math.floor(Math.random() * 20000);
  const child = spawn(binary, [`--user-data-dir=${profileDir}`, `--remote-debugging-port=${port}`,
    '--remote-debugging-address=127.0.0.1', '--no-first-run', '--no-default-browser-check', 'about:blank'],
  { stdio: 'ignore' });
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) return { browser: await chromium.connectOverCDP(`http://127.0.0.1:${port}`), chrome: child };
    } catch { /* still starting */ }
    await new Promise(done => setTimeout(done, 500));
  }
  child.kill();
  throw new Error('Chrome did not start; close any Chrome window using the collector profile and retry');
}

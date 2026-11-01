#!/usr/bin/env node
// Each release's file list, as RuTracker shows it under the torrent ("Список файлов"), so the
// launcher can tell which releases it can install (src/package.rs) and the Library can leave out
// the rest. RuTracker shows file lists only to signed-in users: sign in yourself in the browser
// this opens. Reads the list page only; never fetches a torrent or a game file.
//
//   node files.mjs --platform linux [--catalog dist/rutracker/linux-topics.json.ru-backup.json]
//                  [--attach PORT] [--limit N] [--delay 1500] [--login-timeout 300]
//
// A list is stored with the torrent's info hash: a re-uploaded release (new hash) is read again.
import { readFile, rename, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { platformFrom } from './platforms.mjs';

/** A list this long is cut: enough to judge a release, small enough for the catalog. */
export const MAX_FILES = 2000;

const entities = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };
const decode = text => text.replace(/&(#x[\da-f]+|#\d+|[a-z]+);/gi, (match, name) =>
  name[0] === '#' ? String.fromCodePoint(name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : Number(name.slice(1)))
    : entities[name.toLowerCase()] ?? match);

/** viewtorrent.php's tree as [[path, bytes]…]: folders are `<li class="dir">` with their own
 * `<ul>`, files `<li>` with the size in `<i>`. The torrent's own folder ("./Name", the top of a
 * multi-file torrent) is left out: downloads are saved without it. Null if it isn't a tree. */
export function parseFileTree(html) {
  if (!/<ul class="ftree">/.test(html)) return null;
  const files = [];
  const folders = [];
  const token = /<li class="dir"><div><b>([^<]*)<\/b>|<li><div><b>([^<]*)<\/b>(?:<s>[^<]*<\/s>)?<i>(\d+)<\/i>|<\/ul>/g;
  let depth = 0; // open <ul>s below the tree's own
  for (const match of html.matchAll(token)) {
    if (match[1] !== undefined) { folders.push(decode(match[1])); depth++; }
    else if (match[2] !== undefined) files.push([[...folders, decode(match[2])].join('/'), Number(match[3])]);
    else if (depth > 0) { folders.pop(); depth--; }
  }
  const top = files.length && files.every(([path]) => path.startsWith('./') && path.includes('/', 2))
    ? files[0][0].slice(0, files[0][0].indexOf('/', 2) + 1) : null;
  return files.map(([path, size]) => {
    let name = top && path.startsWith(top) ? path.slice(top.length) : path;
    if (name.startsWith('./')) name = name.slice(2);
    return [name, size];
  });
}

/** The magnet's info hash, upper case. */
export const infoHash = magnet => /urn:btih:([a-z\d]+)/i.exec(magnet || '')?.[1]?.toUpperCase() ?? null;

/** Topics whose list is missing or belongs to an older upload. */
export const needsList = topic => Boolean(infoHash(topic.magnet)) && topic.files?.hash !== infoHash(topic.magnet);

/** What's kept for a topic: its hash, when it was read, and the (possibly cut) list. */
export function fileRecord(topic, list, now = new Date()) {
  return { hash: infoHash(topic.magnet), collected_at: now.toISOString(), count: list.length,
    bytes: list.reduce((sum, [, size]) => sum + size, 0), truncated: list.length > MAX_FILES, list: list.slice(0, MAX_FILES) };
}

/** Runs in the signed-in RuTracker page: the raw list page for each topic. */
async function fetchLists(ids) {
  const out = [];
  for (const id of ids) {
    try {
      const response = await fetch('/forum/viewtorrent.php', { method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: `t=${id}`, signal: AbortSignal.timeout(45_000) });
      const charset = /charset\s*=\s*["']?([^\s;"']+)/i.exec(response.headers.get('content-type') || '')?.[1] || 'windows-1251';
      out.push({ id, status: response.status, html: new TextDecoder(charset).decode(await response.arrayBuffer()) });
    } catch (error) {
      out.push({ id, status: 0, html: '', error: String(error) });
    }
  }
  return out;
}

async function main(args) {
  const option = (name, fallback) => {
    const i = args.indexOf(name);
    return i < 0 ? fallback : args[i + 1];
  };
  const platform = platformFrom(args);
  const path = resolve(option('--catalog', fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json.ru-backup.json`, import.meta.url))));
  const limit = Number(option('--limit', 'Infinity'));
  const delay = Number(option('--delay', '1500'));
  const loginSeconds = Number(option('--login-timeout', '300'));
  const catalog = JSON.parse(await readFile(path, 'utf8'));
  const todo = catalog.topics.filter(needsList).slice(0, limit);
  console.log(`${todo.length} of ${catalog.topics.length} ${platform.label} releases need their file list.`);
  if (!todo.length) return;

  const { chromium } = await import('playwright-core');
  const attach = option('--attach', null);
  let browser, chrome;
  if (attach) browser = await chromium.connectOverCDP(`http://127.0.0.1:${Number(attach)}`);
  else ({ browser, chrome } = await (await import('./browser.mjs')).startChrome(resolve(homedir(), '.cache/azrak/rutracker-list-browser')));
  const context = browser.contexts()[0];
  const page = context.pages().find(p => p.url().startsWith('https://rutracker.net/')) || await context.newPage();
  const save = async () => {
    const temporary = `${path}.${process.pid}.tmp`;
    await writeFile(temporary, JSON.stringify(catalog, null, 2) + '\n');
    await rename(temporary, path);
  };
  try {
    if (!page.url().startsWith('https://rutracker.net/')) await page.goto(platform.source, { waitUntil: 'domcontentloaded', timeout: 45_000 });
    const signedIn = async () => parseFileTree((await page.evaluate(fetchLists, [todo[0].id]))[0].html) !== null;
    if (!await signedIn()) {
      console.log(`Sign in to RuTracker in the browser window (waiting up to ${loginSeconds} s)…`);
      const until = Date.now() + loginSeconds * 1000;
      while (!await signedIn()) {
        if (Date.now() > until) throw new Error('Not signed in to RuTracker; file lists are shown to signed-in users only');
        await new Promise(done => setTimeout(done, 5000));
      }
    }
    let done = 0, failed = 0;
    for (const topic of todo) {
      const [result] = await page.evaluate(fetchLists, [topic.id]);
      const list = parseFileTree(result.html);
      if (list) { topic.files = fileRecord(topic, list); done++; }
      else {
        failed++;
        console.log(`  ${topic.id}: no file list (HTTP ${result.status}${result.error ? `, ${result.error}` : ''}): ${result.html.slice(0, 80).replace(/\s+/g, ' ')}`);
        if (/Not logged in/i.test(result.html)) throw new Error('Signed out of RuTracker');
      }
      if ((done + failed) % 25 === 0) { await save(); console.log(`${done + failed}/${todo.length} read (${failed} without a list)`); }
      await new Promise(next => setTimeout(next, delay));
    }
    await save();
    console.log(`Done: ${done} file lists read, ${failed} without one. Saved ${path}`);
  } finally {
    await save().catch(() => {});
    if (chrome) { await browser.close().catch(() => {}); chrome.kill(); }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(error => { console.error(error.message); process.exitCode = 1; });
}

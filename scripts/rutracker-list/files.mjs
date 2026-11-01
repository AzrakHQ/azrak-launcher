#!/usr/bin/env node
// Each release's file list, as RuTracker shows it under the torrent ("Список файлов"), so the
// launcher can tell which releases it can install (src/package.rs) and the Library can leave out
// the rest. RuTracker shows file lists only to signed-in users: sign in yourself in the browser
// this opens. Reads the list page only; never fetches a torrent or a game file.
//
//   node files.mjs --platform linux [--catalog dist/rutracker/linux-topics.json.ru-backup.json]
//                  [--attach PORT] [--limit N] [--delay 1500] [--login-timeout 300]
//   node files.mjs --merge FROM.json INTO.json    (publish-files.sh uses it)
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

/** Game data, by extension: never what starts a game nor what makes a release uninstallable
 * (src/package.rs looks at archives, scripts, programs, packages and images). */
const DATA = new Set(('dll so pak pck ogg wav mp3 flac opus wma png jpg jpeg bmp tga dds gif webp ico svg ttf otf fon '
  + 'txt md pdf html htm css js json xml ini cfg conf lua py pyc nls mo po dat assets ress resource bank fsb bik bk2 '
  + 'usm webm mp4 avi mkv ogv wmv mov wem bnk pdb manifest cat inf reg tex mdl bsp uasset uexp ubulk utoc ucas mesh anim '
  + 'shader spv glsl hlsl fx sav log db sqlite csv yml yaml toml strings loc lng font xnb xwb xsb wad pk3 pk4 vpk '
  + 'forge sbf pff mtl obj fbx dae ani cur rtf doc docx xls cs h c cpp hpp mdmp crt pem cer a def res sys idl bundle').split(' '));
const extension = path => {
  const name = path.slice(path.lastIndexOf('/') + 1).toLowerCase();
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1) : '';
};
const isData = path => /\.so(\.\d+)+$/i.test(path) || DATA.has(extension(path));
/** Could start a game, loosely (more than package.rs accepts): a script, a program, an .exe, a
 * file without a real extension. */
const mayStart = path => {
  const ext = extension(path);
  return !ext || ext.length > 5 || /^\d+$/.test(ext) || /[^a-z0-9]/.test(ext)
    || ['sh', 'run', 'appimage', 'exe', 'x86_64', 'x86', 'x64', 'bin', 'amd64', 'i386', 'i686'].includes(ext);
};
const depth = path => (path.match(/\//g) || []).length;

/** What's published of a list, small enough for the catalog: everything at the top and one
 * folder down, game data left out two folders down, nothing deeper; `deeper_starters` says
 * whether something left out could start the game (then the list can't show a release can't be
 * installed). Applying it again changes nothing. */
export function compact(files) {
  const deeper = files.list.some(([path]) => depth(path) >= 3 && mayStart(path));
  const list = files.list.filter(([path]) => depth(path) <= 1 || (depth(path) === 2 && !isData(path)));
  return { ...files, list, deeper_starters: Boolean(files.deeper_starters) || deeper };
}

/** The magnet's info hash, upper case. */
export const infoHash = magnet => /urn:btih:([a-z\d]+)/i.exec(magnet || '')?.[1]?.toUpperCase() ?? null;

/** Topics whose list is missing or belongs to an older upload. */
export const needsList = topic => Boolean(infoHash(topic.magnet)) && topic.files?.hash !== infoHash(topic.magnet);

/** What's kept for a topic: its hash, when it was read, how many files it has, and its compact
 * list (cut if it's still too long). */
export function fileRecord(topic, list, now = new Date()) {
  const files = compact({ list });
  return { hash: infoHash(topic.magnet), collected_at: now.toISOString(), count: list.length,
    bytes: list.reduce((sum, [, size]) => sum + size, 0), truncated: files.list.length > MAX_FILES,
    deeper_starters: files.deeper_starters, list: files.list.slice(0, MAX_FILES) };
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

/** Copy `from`'s file lists into `into` (another copy of the catalog, such as the published one),
 * where it's the same topic and torrent. Returns how many were copied. */
export function mergeFileLists(from, into) {
  const lists = new Map(from.topics.filter(t => t.files).map(t => [t.id, t.files]));
  let merged = 0;
  for (const topic of into.topics) {
    const files = lists.get(topic.id);
    if (!files || files.hash !== infoHash(topic.magnet)) continue;
    const published = compact(files);
    if (JSON.stringify(topic.files) !== JSON.stringify(published)) { topic.files = published; merged++; }
  }
  return merged;
}

async function main(args) {
  const option = (name, fallback) => {
    const i = args.indexOf(name);
    return i < 0 ? fallback : args[i + 1];
  };
  if (args[0] === '--merge') {
    const [from, into] = [args[1], args[2]].map(path => resolve(path));
    const target = JSON.parse(await readFile(into, 'utf8'));
    const merged = mergeFileLists(JSON.parse(await readFile(from, 'utf8')), target);
    await writeFile(`${into}.tmp`, JSON.stringify(target, null, 2) + '\n');
    await rename(`${into}.tmp`, into);
    console.log(`${merged} file lists merged into ${into} (${target.topics.filter(t => t.files).length} of ${target.topics.length} releases have one)`);
    return;
  }
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
  // An attached browser's connection would keep the process alive.
  main(process.argv.slice(2)).then(() => process.exit(0), error => { console.error(error.message); process.exit(1); });
}

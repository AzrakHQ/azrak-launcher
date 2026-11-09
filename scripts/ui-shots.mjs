#!/usr/bin/env node
// Screenshot matrix: the main screens (Home and its top bar, every Settings section, the
// Downloads page) at several window sizes, in the same offline sandbox as test-downloads.mjs.
// Each screenshot is compared with the last approved one; a contact sheet per screen puts every
// size side by side for a look (by a person, or a model asked to check for cut or overlapping
// text, misalignment and things that move between states).
//
// Usage: node scripts/ui-shots.mjs [--sizes 1280x720,1920x1080] [--art] [--approve] [binary]
//   --art      use this PC's image cache (~/.cache/azrak, read only) so screens have key art
//   --approve  make these screenshots the approved ones (target/ui-golden)
// Output: target/ui-shots/<size>/<screen>.png, target/ui-shots/sheets/<screen>.png, and a
// diff image beside each screenshot that changed. Exit 1 when a screen changed.
// Needs unshare, Xvfb, xdotool, ffmpeg and ImageMagick. Never starts a torrent or runs a game.
import { spawn, spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync, chmodSync, copyFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(import.meta.url);
const root = resolve(dirname(script), '..');
const args = process.argv.slice(2);
const flag = name => args.includes(name);
const option = name => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : null; };
const binary = resolve(args.find((a, i) => !a.startsWith('--') && args[i - 1] !== '--sizes') || join(root, 'target/release/azrak'));
// Odd sizes on purpose: a window that isn't 16:9 or a round number is where layouts break.
const sizes = (option('--sizes') || '1280x720,1366x768,1600x900,1737x1047,1920x1080,2560x1440')
  .split(',').map(s => s.split('x').map(Number));

if (process.env.AZRAK_UI_SHOTS_ISOLATED !== '1') {
  const [w, h] = sizes.reduce(([a, b], [c, d]) => [Math.max(a, c), Math.max(b, d)], [0, 0]);
  const result = spawnSync('unshare', ['--user', '--map-root-user', '--net', 'xvfb-run', '-a',
    '-s', `-screen 0 ${w}x${h}x24`, process.execPath, script, ...args], {
    stdio: 'inherit', timeout: 30 * 60_000, env: { ...process.env, AZRAK_UI_SHOTS_ISOLATED: '1', REAL_HOME: homedir() },
  });
  process.exit(result.status ?? 1);
}

const out = join(root, 'target/ui-shots');
const golden = join(root, 'target/ui-golden');
const profile = join(out, 'profile');
rmSync(out, { recursive: true, force: true });
const sleep = ms => new Promise(r => setTimeout(r, ms));
const run = (cmd, a) => spawnSync(cmd, a, { encoding: 'utf8' });

/// A fresh profile: one installed game, and downloads in the states the page shows.
function makeProfile() {
  rmSync(profile, { recursive: true, force: true });
  const cfg = join(profile, 'config/azrak'), cache = join(profile, 'cache/azrak'), store = join(cfg, 'transfers');
  const games = join(profile, 'games'), base = join(profile, 'downloads');
  for (const d of [store, cache, join(profile, 'data'), games, base]) mkdirSync(d, { recursive: true });
  chmodSync(store, 0o700);
  const realCache = join(process.env.REAL_HOME || homedir(), '.cache/azrak');
  const art = flag('--art') && existsSync(realCache);
  if (art) {
    for (const f of readdirSync(realCache)) {
      const p = join(realCache, f);
      if (statSync(p).isFile()) copyFileSync(p, join(cache, f));
    }
    symlinkSync(join(realCache, 'img'), join(cache, 'img'));
    // The published catalogs those caches came from (else the cache is set aside for the bundled list).
    const realLists = join(process.env.REAL_HOME || homedir(), '.local/share/azrak/rutracker');
    if (existsSync(realLists)) cpSync(realLists, join(profile, 'data/azrak/rutracker'), { recursive: true });
  }
  // Fairy Tail 2's title, so Home has its key art when the cache has it.
  mkdirSync(join(games, 'Game/sce_sys'), { recursive: true });
  writeFileSync(join(games, 'Game/sce_sys/param.json'), JSON.stringify({ titleId: art ? 'CUSA49633' : 'PPSA12345',
    contentVersion: '01.00', localizedParameters: { defaultLanguage: 'en-US', 'en-US': { titleName: art ? 'Fairy Tail 2' : 'Generated game' } } }));
  writeFileSync(join(cfg, 'config.json'), JSON.stringify({ sounds: false, game_dirs: [games], download_dir: base,
    install_dir: join(profile, 'library'), kyty_auto_update: false, app_auto_update: false, shad_auto_update: false }));
  const job = (n, name, state, done, total, extra = {}) => {
    const key = String(n).repeat(40), folder = join(base, `torrent-${key}`);
    mkdirSync(folder, { recursive: true });
    writeFileSync(join(folder, '.azrak-owned'), key);
    // Byte counts are whole numbers (a fraction makes the launcher refuse the whole list).
    return { key, topic: topics[n - 1], name, magnet: `magnet:?xt=urn:btih:${key}`, folder, state,
      done: Math.round(done), total: Math.round(total),
      file_count: 1, files: [], error: '', platform: 'Ps4', added: 1_700_000_000 - n * 1000, ...extra };
  };
  const G = 2 ** 30;
  // Catalog topics (with --art, their key art and logos): Fairy Tail 2, Chaos On Deponia, The
  // Exit Liminal Anomaly Horror, The Sinking City 2. The first is the newest (shown first).
  const topics = [6618049, 5611591, 6699170, 6929281];
  writeFileSync(join(store, 'jobs.json'), JSON.stringify({ schema: 1, jobs: [
    job(1, 'Fairy Tail 2 Ultimate Edition', 'Paused', 0, 11.6 * G),
    job(2, 'A download at sixty-one percent', 'Paused', 7.1 * G, 11.6 * G),
    job(3, 'A download that stopped with a long reason', 'Failed', 1.3 * G, 1.64 * G, { error: 'Not enough free space in the download folder' }),
    job(4, 'A cancelled download', 'Cancelled', 2.2 * G, 9.5 * G),
  ] }));
}

/// The screens, as keys pressed from Home; a name after each step is a screenshot.
const steps = [
  ['home', []],
  ['top-downloads', ['Up', 'Up', 'Right', 'Right']],
  ['top-settings', ['Right']],
  ['settings', ['s']],
  ...[1, 2, 3, 4, 5, 6].map(i => [`settings-${i}`, ['Down']]),
  ['settings-quit', ['Down']],
  ['settings-rows', ['Up', 'Up', 'Up', 'Up', 'Up', 'Right']],
  // Back from the rows to the sections, then closed.
  ['downloads', ['Escape', 'Escape', 'ctrl+d']],
  ['downloads-2', ['Right']],
  ['downloads-3', ['Right']],
  ['downloads-buttons', ['Up']],
  // With --art (this PC's catalog): a Game Hub whose release name says more than its title.
  ...(flag('--art') ? [
    ['library', ['Escape', 'Escape', 'Escape', 'Tab']],
    ['library-search', [...'blacklist'.split(''), 'Return']],
    ['hub-release', ['Return']],
  ] : []),
];

async function shoot([w, h]) {
  const dir = join(out, `${w}x${h}`);
  mkdirSync(dir, { recursive: true });
  makeProfile();
  const env = { ...process.env, XDG_CONFIG_HOME: join(profile, 'config'), XDG_CACHE_HOME: join(profile, 'cache'),
    XDG_DATA_HOME: join(profile, 'data'), LIBGL_ALWAYS_SOFTWARE: '1', SLINT_BACKEND: 'winit-femtovg' };
  const child = spawn(binary, ['--windowed'], { env, stdio: ['ignore', 'ignore', 'pipe'] });
  let log = '';
  child.stderr.on('data', d => { log += d; });
  const exited = new Promise(r => child.on('exit', r));
  for (let i = 0; i < 120 && !log.includes('home screen shown') && child.exitCode === null; i++) await sleep(250);
  if (!log.includes('home screen shown')) {
    child.kill();
    console.error(`${w}x${h}: the launcher didn't reach its home screen\n${log.slice(-2000)}`);
    process.exit(1);
  }
  // Found within 20 s, or the run stops (keys must never go to some other window).
  const found = spawnSync('xdotool', ['search', '--sync', '--onlyvisible', '--name', '^Azrak Launcher$'], { encoding: 'utf8', timeout: 20_000 });
  const win = (found.stdout || '').trim().split('\n')[0];
  if (!win) {
    child.kill();
    console.error(`${w}x${h}: no window titled "Azrak Launcher" appeared`);
    process.exit(1);
  }
  run('xdotool', ['windowfocus', '--sync', win]);
  run('xdotool', ['windowsize', win, String(w), String(h)]);
  run('xdotool', ['windowmove', win, '0', '0']);
  run('xdotool', ['mousemove', String(w - 2), String(h - 2)]);
  await sleep(3000);
  for (const [name, keys] of steps) {
    if (keys.length) run('xdotool', ['key', '--delay', '160', ...keys]);
    await sleep(name === 'settings' || name === 'downloads' || name.startsWith('hub') ? 1600 : 900);
    run('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab', '-draw_mouse', '0',
      '-video_size', `${w}x${h}`, '-i', `${process.env.DISPLAY}+0,0`, '-frames:v', '1', join(dir, `${name}.png`)]);
  }
  if (child.exitCode !== null) {
    console.error(`${w}x${h}: the launcher closed while its screens were taken\n${log.slice(-2000)}`);
    process.exit(1);
  }
  child.kill();
  await Promise.race([exited, sleep(5000)]);
}

for (const size of sizes) {
  console.log(`${size.join('×')}…`);
  await shoot(size);
}
rmSync(profile, { recursive: true, force: true });

// Contact sheets: each screen at every size, side by side.
mkdirSync(join(out, 'sheets'), { recursive: true });
for (const [name] of steps) {
  // Each picture with its own size (one missing doesn't shift the labels).
  const files = sizes.map(([w, h]) => [`${w}×${h}`, join(out, `${w}x${h}`, `${name}.png`)]).filter(([, f]) => existsSync(f));
  run('montage', [...files.flatMap(([label, f]) => ['-label', label, f]),
    '-tile', '3x', '-geometry', '800x450+6+6', '-pointsize', '18', '-background', '#222', '-fill', '#ddd',
    join(out, 'sheets', `${name}.png`)]);
}

// Against the approved screenshots: more than a sliver changed (the clock alone is allowed).
let changed = 0, compared = 0, approved = 0;
for (const [w, h] of sizes) for (const [name] of steps) {
  const shot = join(out, `${w}x${h}`, `${name}.png`), ok = join(golden, `${w}x${h}`, `${name}.png`);
  if (!existsSync(shot)) { changed++; console.log(`missing: ${w}x${h}/${name}`); continue; }
  if (flag('--approve')) { mkdirSync(dirname(ok), { recursive: true }); cpSync(shot, ok); approved++; continue; }
  if (!existsSync(ok)) continue;
  compared++;
  const r = run('compare', ['-metric', 'AE', '-fuzz', '6%', ok, shot, shot.replace(/\.png$/, '.diff.png')]);
  // A comparison that couldn't be made (other sizes, no ImageMagick) is a change, not a pass.
  const pixels = Number.parseFloat(r.stderr);
  if (!Number.isFinite(pixels)) {
    changed++;
    console.log(`couldn't compare: ${w}x${h}/${name}: ${(r.stderr || r.error?.message || '').trim()}`);
  } else if (pixels / (w * h) > 0.004) {
    changed++;
    console.log(`changed: ${w}x${h}/${name} (${(100 * pixels / (w * h)).toFixed(2)}% of pixels)`);
  } else rmSync(shot.replace(/\.png$/, '.diff.png'), { force: true });
}
console.log(flag('--approve') ? `approved ${approved} screenshots` : compared
  ? `${changed} of ${compared} screens changed since they were approved` : 'nothing approved yet: look at the sheets, then run with --approve');
console.log(`sheets: ${join(out, 'sheets')}`);
process.exit(changed ? 1 : 0);

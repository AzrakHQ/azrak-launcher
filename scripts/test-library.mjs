#!/usr/bin/env node
// Native Linux Library regression/evidence test. Run AFTER building the release binary:
//   node scripts/test-library.mjs [path/to/azrak] [--full-hd]
// No builds, real profiles, real game files, HTTP clients or torrent actions are used.
// All artwork and game metadata below are authored fixtures, not retail payloads.
import assert from 'node:assert/strict';
import { spawn, execFile as execCallback } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, chmod, copyFile, mkdir, mkdtemp, readFile, readlink, readdir, writeFile } from 'node:fs/promises';
import { writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execCallback);
const script = fileURLToPath(import.meta.url);
const root = resolve(dirname(script), '..');
const inner = process.argv[2] === '--namespace';
const args = inner ? process.argv.slice(5) : process.argv.slice(2);
assert.ok(args.filter(arg => !arg.startsWith('--')).length <= 1, 'Supply at most one binary path');
assert.ok(args.filter(arg => arg.startsWith('--')).every(arg => arg === '--full-hd'), 'Only --full-hd is supported');
const binary = resolve(args.find(arg => !arg.startsWith('--')) || join(root, 'target/release/azrak'));
const fullHD = args.includes('--full-hd');
const normalize = text => text.replace(/\s+/g, ' ').trim();
const sha1 = value => createHash('sha1').update(value).digest('hex');
const sha256 = value => createHash('sha256').update(value).digest('hex');

// Never kill by executable/window name: only process groups created by this script.
function signalGroup(child, signal) {
  if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
  try { process.kill(-child.pid, signal); } catch (error) { if (error.code !== 'ESRCH') throw error; }
}

async function outer() {
  assert.equal(process.platform, 'linux', 'This native test requires Linux');
  const artifacts = await mkdtemp(join(tmpdir(), 'ps5-library-ui-'));
  console.log(`Library artifacts (retained on failure too): ${artifacts}`);
  const namespace = JSON.stringify({ net: await readlink('/proc/self/ns/net'), pid: await readlink('/proc/self/ns/pid') });
  // Private /proc also prevents the production session monitor from detecting
  // a real emulator and reading its game metadata outside our synthetic library.
  // Xvfb must not have PID 1 as its parent: it suppresses the SIGUSR1 ready
  // notification in that case, leaving xvfb-run blocked. Isolate the launcher
  // process tree only after the private X server is ready.
  const child = spawn('unshare', ['--user', '--map-root-user', '--net', 'xvfb-run', '-a',
    '-s', '-screen 0 1920x1080x24 -nolisten tcp', 'unshare', '--mount', '--pid', '--fork', '--kill-child=SIGKILL', '--mount-proc', process.execPath, script,
    '--namespace', artifacts, namespace, ...args], { detached: true, stdio: 'inherit' });
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    console.error('Library test exceeded its outer 180-second deadline');
    signalGroup(child, 'SIGTERM');
  }, 180_000);
  const killer = setTimeout(() => signalGroup(child, 'SIGKILL'), 185_000);
  const interrupt = () => signalGroup(child, 'SIGTERM');
  process.once('SIGINT', interrupt);
  process.once('SIGTERM', interrupt);
  try {
    const code = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', code => resolve(code));
    });
    process.exitCode = timedOut ? 1 : code ?? 1;
    if (process.exitCode) console.error('Requires unshare user/network namespaces, Xvfb/xauth, xdotool, ffmpeg with SVG support, Tesseract and ImageMagick. No network-enabled fallback.');
  } finally {
    clearTimeout(timer);
    clearTimeout(killer);
    process.off('SIGINT', interrupt);
    process.off('SIGTERM', interrupt);
  }
}

async function test() {
  const artifacts = resolve(process.argv[3]);
  const parentNamespace = JSON.parse(process.argv[4]);
  assert.notEqual(await readlink('/proc/self/ns/net'), parentNamespace.net, 'Refuse to run outside a new network namespace');
  const pidNamespace = await readlink('/proc/self/ns/pid');
  assert.notEqual(pidNamespace, parentNamespace.pid, 'Refuse to expose real emulator processes to the session monitor');
  assert.equal(await readlink('/proc/1/ns/pid'), pidNamespace, 'Require a namespace-private /proc mount');
  // /proc/net is namespace-relative; an inherited sysfs mount can still expose
  // the parent's interface names even after unshare --net.
  const interfaces = (await readFile('/proc/net/dev', 'utf8')).split('\n').slice(2)
    .filter(line => line.includes(':')).map(line => line.split(':')[0].trim()).sort();
  assert.deepEqual(interfaces, ['lo'], 'Namespace must have no external network interface');
  assert.ok(process.env.DISPLAY && process.env.XAUTHORITY, 'Private Xvfb display/authentication required');
  await access(binary);
  for (const command of ['xdotool', 'ffmpeg', 'tesseract', 'convert', 'identify']) {
    await exec('which', [command], { timeout: 5_000 });
  }

  const base = join(artifacts, 'profile');
  const configDir = join(base, 'config/azrak');
  const cacheDir = join(base, 'cache/azrak');
  const configPath = join(configDir, 'config.json');
  const sourcePath = join(artifacts, 'catalog.json');
  const installRoot = join(base, 'generated-library');
  const gamePath = join(installRoot, 'inert-fixture');
  const emulator = join(base, 'not-an-emulator');
  const names = ['Velvet Signal', 'Aster Circuit', 'Copper Orbit', 'Drift Atlas', 'Ember Gate', 'Fjord Echo',
    'Garnet Trail', 'Harbor Prism', 'Indigo Relay', 'Jade Summit', 'Kestrel Loop', 'Lumen Field',
    'Moonlit Passage Beyond the Glass Horizon', 'Nimbus Cove', 'Opal Engine', 'Paper Aurora',
    'Quartz Meadow', 'Ribbon Flight', 'Solar Haven', 'Tide Garden', 'Umber Route', 'Vivid Maze',
    'Willow Arc', 'Zenith Vale'];
  const genres = ['Action', 'Adventure', 'Puzzle', 'Racing'];
  const sorts = [/Newest topics/i, /Name\s*\(\s*A\s*[–—-]?\s*Z\s*\)/i, /Release date/i, /Top rated/i,
    /Size\s*\(largest\)/i, /Size\s*\(smallest\)/i, /KytyPS5 compatibility/i];
  const snapshot = '2026-09-30T00:56:18.731Z';
  const checks = [];
  const images = [];
  const pass = label => { checks.push(label); console.log(`PASS ${label}`); };
  let active;
  let deadline;

  // Whitelist environment rather than inheriting profile locations, proxies, API keys,
  // autostart options, artwork-disable flags, Wayland or a session D-Bus connection.
  const env = {
    PATH: '/usr/local/bin:/usr/bin:/bin', HOME: join(base, 'home'),
    XDG_CONFIG_HOME: join(base, 'config'), XDG_CACHE_HOME: join(base, 'cache'),
    XDG_DATA_HOME: join(base, 'data'), XDG_STATE_HOME: join(base, 'state'),
    XDG_RUNTIME_DIR: join(base, 'runtime'), TMPDIR: join(base, 'tmp'),
    DISPLAY: process.env.DISPLAY, XAUTHORITY: process.env.XAUTHORITY,
    LANG: 'C.UTF-8', LC_ALL: 'C.UTF-8', TZ: 'UTC',
    LIBGL_ALWAYS_SOFTWARE: '1', SLINT_BACKEND: 'winit-femtovg', SLINT_SCALE_FACTOR: '1',
    WINIT_UNIX_BACKEND: 'x11',
  };
  for (const path of [configDir, cacheDir, join(gamePath, 'sce_sys'), join(artifacts, 'posters'),
    env.HOME, env.XDG_DATA_HOME, env.XDG_STATE_HOME, env.XDG_RUNTIME_DIR, env.TMPDIR]) {
    await mkdir(path, { recursive: true });
  }
  await chmod(env.XDG_RUNTIME_DIR, 0o700);
  // Existing non-executable file prevents Config::load's real emulator auto-detection.
  await writeFile(emulator, 'Generated inert sentinel; never execute.\n', { mode: 0o600 });
  await writeFile(configPath, JSON.stringify({ emulator, game_dirs: [installRoot],
    download_dir: join(base, 'unused-downloads'), install_dir: join(base, 'unused-installation'),
    library_compact: false, sounds: false, fullscreen: false, rawg_key: '', rawg_art: [],
    kyty_auto_update: false, app_auto_update: false, extra_args: '' }));
  await writeFile(join(gamePath, 'sce_sys/param.json'), JSON.stringify({ titleId: 'PPSA98000',
    contentVersion: '01.100.000', localizedParameters: { defaultLanguage: 'en-US',
      'en-US': { titleName: names[0] } } }));
  const ebootPath = join(gamePath, 'eboot.bin');
  await writeFile(ebootPath, Buffer.alloc(4096, 42), { mode: 0o600 });
  const ebootDigest = sha256(await readFile(ebootPath));

  const bundled = JSON.parse(await readFile(join(root, 'assets/rutracker/ps5-topics.json'), 'utf8'));
  assert.equal(bundled.complete, true);
  assert.equal(bundled.translation.language, 'en');
  assert.equal(bundled.topic_count, bundled.topics.length);
  assert.equal(bundled.topic_count, 614, 'Bundled release-model baseline changed; review this test');
  // Borrow the validated header only, never the bundled game records/magnets/artwork.
  const topics = names.map((name, index) => ({ id: String(9024 - index), title: `[PS5] ${name}`,
    size: `${index === 1 ? 1 : index === 22 ? 99 : 12 + index} GB`,
    seeders: index === 1 ? null : index === 2 ? 0 : 22 + index,
    leechers: index === 1 ? null : index === 2 ? 0 : 7,
    magnet: `magnet:?xt=urn:btih:${sha1(`authored-library-fixture-${index}`)}`,
    game_info: { name, title_id: `PPSA${98000 + index}`, genre: genres[index % genres.length],
      version: '1.100', region: 'USA', release_date: index === 3 ? '2026-09-29' : `2020-01-${String(index + 1).padStart(2, '0')}`,
      image_urls: [`https://fixture.invalid/image-${index}.png`],
      release_summary: 'Original native UI fixture; no retail game data or executable code.' } }));
  topics.push({ ...topics[0], id: '10000', title: `[PS5] ${names[0]} Deluxe Edition`,
    magnet: `magnet:?xt=urn:btih:${sha1('authored-alternative-edition')}`,
    game_info: { ...topics[0].game_info, name: `${names[0]} Deluxe Edition`, version: '1.000', region: 'EUR' } });
  await writeFile(sourcePath, JSON.stringify({ source: bundled.source, complete: true,
    translation: { language: 'en' }, collected_at: snapshot, topic_count: topics.length, topics }));

  // Fresh negative PSN entries prevent enrichment calls while leaving g.cover as
  // the image source. No PSN Info, CDN sizing variant, real cache or credentials.
  const psn = Object.fromEntries(names.map((_, index) => [`PPSA${98000 + index}`, { at: Date.now() / 1000, info: null }]));
  const psnPath = join(cacheDir, 'psn.json');
  await writeFile(psnPath, JSON.stringify(psn));
  const psnDigest = sha256(await readFile(psnPath));
  const compatibility = Object.fromEntries(names.map((_, index) => {
    const status = [0, 4, 6].includes(index) ? 'InGame' : index % 2 ? 'Logo' : 'MainMenu';
    return [`PPSA${98000 + index}`, { status, reports: 1,
      platforms: { linux: { status, version: 'Generated UI fixture 2026-09-30' } } }];
  }));
  const compatibilityPath = join(cacheDir, 'compatibility.json');
  await writeFile(compatibilityPath, JSON.stringify(compatibility));
  const compatibilityDigest = sha256(await readFile(compatibilityPath));

  // Original abstract poster art. A pale horizontal stripe also provides a
  // deterministic pixel ruler for visible column counts (independent of OCR).
  for (let index = 0; index < names.length; index++) {
    const hue = (index * 37 + 205) % 360;
    const svg = join(artifacts, 'posters', `${index}.svg`);
    const png = join(artifacts, 'posters', `${index}.png`);
    await writeFile(svg, `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="900" viewBox="0 0 600 900">
      <defs><linearGradient id="g" x2="1" y2="1"><stop stop-color="hsl(${hue},65%,24%)"/>
      <stop offset="1" stop-color="#091323"/></linearGradient></defs>
      <rect width="600" height="900" fill="url(#g)"/><rect y="80" width="600" height="100" fill="#bceaf4"/>
      <circle cx="300" cy="450" r="170" fill="none" stroke="hsl(${hue},85%,70%)" stroke-width="4"/>
      <circle cx="300" cy="450" r="126" fill="none" stroke="#bceaf4" stroke-opacity=".4" stroke-width="2"/>
      <path d="M300 256 L446 515 L300 642 L154 515 Z" fill="hsl(${hue},70%,55%)" fill-opacity=".65"/>
      <path d="M300 314 L394 486 L300 576 L206 486 Z" fill="#bceaf4" fill-opacity=".8"/>
      <circle cx="300" cy="450" r="40" fill="#102238"/>
      <path d="M70 720 H530 M70 738 H390 M70 756 H450" stroke="#bceaf4" stroke-opacity=".35" stroke-width="3"/>
      <circle cx="500" cy="820" r="${16 + index}" fill="hsl(${hue},85%,70%)"/>
      </svg>`);
    await exec('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-i', svg, '-frames:v', '1', png], { timeout: 10_000 });
    const hash = sha1(topics[index].game_info.image_urls[0]);
    const rawDir = join(cacheDir, 'img', hash.slice(0, 2));
    await mkdir(rawDir, { recursive: true });
    await copyFile(png, join(rawDir, hash));
  }
  pass('24 original poster caches, 24 unique games / 25 releases, zero/unknown peers and one inert installed fixture');

  function waitLog(session, marker) {
    return new Promise((resolve, reject) => {
      let finished = false;
      const timer = setTimeout(() => finish(new Error(`No ${marker} within 20 seconds; see ${session.name}.log`)), 20_000);
      const check = () => { if (session.logs.includes(marker)) finish(); };
      const exited = code => finish(new Error(`Launcher exited (${code}) before ${marker}: ${session.logs}`));
      function finish(error) {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        session.child.stderr.off('data', check);
        session.child.off('exit', exited);
        session.child.off('error', finish);
        error ? reject(error) : resolve();
      }
      session.child.stderr.on('data', check);
      session.child.once('exit', exited);
      session.child.once('error', finish);
      if (session.child.exitCode !== null || session.child.signalCode !== null) exited(session.child.exitCode);
      else check();
    });
  }

  async function stop(session, graceful = false) {
    if (!session) return;
    if (!graceful) signalGroup(session.child, 'SIGTERM');
    const killer = setTimeout(() => signalGroup(session.child, 'SIGKILL'), graceful ? 10_000 : 3_000);
    try {
      await session.closed;
      if (graceful) assert.equal(session.child.exitCode, 0, 'Graceful quit must exit successfully');
    } finally {
      clearTimeout(killer);
      await writeFile(join(artifacts, `${session.name}.log`), session.logs);
      if (active === session) active = undefined;
    }
  }

  const interrupt = () => {
    if (active) {
      writeFileSync(join(artifacts, `${active.name}.log`), active.logs);
      signalGroup(active.child, 'SIGKILL');
    }
    // Emergency cleanup still touches only our generated profile. No app can
    // write over this once its owned process group has been killed.
    writeFileSync(configPath, JSON.stringify({ emulator, game_dirs: [installRoot],
      download_dir: join(base, 'unused-downloads'), install_dir: join(base, 'unused-installation'),
      library_compact: false, sounds: false, fullscreen: false, rawg_key: '', rawg_art: [],
      kyty_auto_update: false, app_auto_update: false, extra_args: '' }));
    writeFileSync(join(artifacts, 'report.json'), JSON.stringify({ binary, checks, images,
      interrupted: true, original_density_restored: true }, null, 2));
    process.exit(1);
  };
  process.once('SIGINT', interrupt);
  process.once('SIGTERM', interrupt);
  // Inner deadline leaves time to terminate the launcher before the outer watchdog.
  deadline = setTimeout(interrupt, 160_000);

  let width = 1349, height = 768;
  const key = (...keys) => exec('xdotool', ['key', '--clearmodifiers', '--delay', '90', ...keys], { timeout: 10_000, env });
  async function resize(w, h) {
    await exec('xdotool', ['windowsize', '--sync', active.window, String(w), String(h)], { timeout: 10_000, env });
    await exec('xdotool', ['windowmove', '--sync', active.window, '0', '0'], { timeout: 10_000, env });
    await exec('xdotool', ['windowfocus', '--sync', active.window], { timeout: 10_000, env });
    width = w; height = h;
  }
  async function start(name, startup = false) {
    assert.equal(active, undefined, 'Do not overlap launcher instances');
    // Config exists: returning splash is automatic. There is no --first flag.
    // Capture early frames before the process starts, not a fake production hook.
    let capture;
    if (startup) {
      capture = exec('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab', '-draw_mouse', '0',
        '-framerate', '10', '-video_size', '1920x1080', '-i', env.DISPLAY,
        '-frames:v', '24', join(artifacts, 'startup-%02d.png')], { timeout: 10_000, env });
      capture.catch(() => {}); // Await below; avoid an unhandled rejection during startup.
    }
    const child = spawn(binary, ['--windowed', '--catalog', sourcePath, '--sync'], {
      detached: true, cwd: base, stdio: ['ignore', 'ignore', 'pipe'], env,
    });
    const session = { child, name, logs: '', closed: null, window: null };
    session.closed = new Promise(resolve => { child.once('close', resolve); child.once('error', resolve); });
    child.stderr.on('data', bytes => { session.logs += bytes; });
    active = session;
    await waitLog(session, 'home screen shown');
    const { stdout } = await exec('xdotool', ['search', '--sync', '--onlyvisible', '--pid', String(child.pid),
      '--name', '^Azrak Launcher$'], { timeout: 10_000, env });
    session.window = stdout.trim().split('\n')[0];
    assert.match(session.window, /^\d+$/);
    await resize(width, height);
    if (capture) await capture;
    await key('Tab'); // Home -> Library, Grid focused. No action/Play is ever confirmed.
  }

  async function shot(name) {
    const path = join(artifacts, `${name}.png`);
    // Twelve real frames at 10fps settle animations; no shell sleeps/poll loops.
    await exec('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab', '-draw_mouse', '0',
      '-framerate', '10', '-video_size', `${width}x${height}`, '-i', env.DISPLAY,
      '-frames:v', '12', '-update', '1', path], { timeout: 10_000, env });
    const { stdout: dimensions } = await exec('identify', ['-format', '%wx%h', path], { timeout: 5_000 });
    assert.equal(dimensions, `${width}x${height}`);
    const results = await Promise.all([3, 11].map(async psm => {
      const { stdout } = await exec('tesseract', [path, 'stdout', '--psm', String(psm)], { timeout: 10_000, env });
      return normalize(stdout);
    }));
    // A thin leading "1" is dropped by whole-window OCR. Read the actual count
    // line separately at larger resolution, keeping strong numeric assertions.
    const scale = geometry(false).scale;
    const countImage = join(artifacts, `${name}-count.png`);
    await exec('convert', [path, '-crop', `${Math.ceil(width * 0.45)}x${Math.ceil(30 * scale)}+${Math.floor(width * 0.55)}+${Math.floor(190 * scale)}`,
      '+repage', '-resize', '300%', '-colorspace', 'Gray', '-negate', '-normalize', countImage], { timeout: 5_000, env });
    const { stdout: countText } = await exec('tesseract', [countImage, 'stdout', '--psm', '7'], { timeout: 10_000, env });
    await writeFile(join(artifacts, `${name}.txt`), `PSM 3\n${results[0]}\n\nPSM 11\n${results[1]}\n\nCount PSM 7\n${normalize(countText)}\n`);
    images.push(`${name}.png`);
    return { path, text: [...results, normalize(countText)].join(' '), passes: results };
  }
  const counts = (shot, filtered = 24) => {
    const expression = filtered === 24 ? /\b24\s+games\s*[-·•.,:]?\s*25\s+releases/i
      : new RegExp(`\\b${filtered}\\s+of\\s+24\\s+games\\s*[-·•.,:]?\\s*25\\s+releases`, 'i');
    assert.match(shot.text, expression, 'Game count must be grouped; total release count must remain 25');
    if (filtered === 24) assert.doesNotMatch(shot.text, /24\s+of\s+24\s+games/i, 'Unfiltered header must not say "of"');
  };
  const toSort = () => key('Home', 'Up', 'Up', 'Up', 'Right'); // Grid -> genre -> status -> Search -> Sort.
  const toGrid = () => key('Down', 'Down', 'Down'); // Sort/density -> status -> genre -> first visible grid card.
  async function chooseSort(current, next) {
    await key('Return');
    if (next > current) await key(...Array(next - current).fill('Down'));
    if (next < current) await key(...Array(current - next).fill('Up'));
    await key('Return'); // Applies and restores Sort focus, rather than confirming any game action.
  }
  async function firstHub(name, expectedTitle) {
    await toGrid();
    await key('Return'); // Z_GRID confirms Game Hub, never Play/Download.
    const hub = await shot(name);
    assert.ok(hub.text.includes(expectedTitle), `Expected first sorted game ${expectedTitle}: ${hub.text}`);
    await key('Escape'); // Back restores Grid focus.
    return hub;
  }

  function geometry(compact) {
    const scale = Math.max(0.3, Math.min(width / 1920, height / 1080));
    const type = Math.max(scale, 0.75);
    const inner = Math.max(width / scale - 192, 400);
    const gap = compact ? 24 : 28;
    const minimum = Math.max(compact ? 190 : 214, (compact ? 126 : 150) / scale);
    const cols = Math.max(2, Math.min(16, Math.floor((inner + gap) / (minimum + gap))));
    const card = (inner - (cols - 1) * gap) / cols * scale;
    return { scale, type, cols, card, gap: gap * scale, top: 350 * scale };
  }
  async function pixels(path, crop) {
    const { stdout } = await exec('convert', [path, '-crop', crop, '+repage', '-alpha', 'off', '-depth', '8', 'RGB:-'],
      { timeout: 5_000, encoding: 'buffer', maxBuffer: 4 * 1024 * 1024 });
    return stdout;
  }
  async function visibleColumns(shot, compact) {
    const g = geometry(compact);
    const row = await pixels(shot.path, `${width}x1+0+${Math.round(g.top + g.card * 0.2)}`);
    const runs = [];
    let start = -1;
    for (let x = 0; x <= width; x++) {
      const bright = x < width && row[x * 3] > 155 && row[x * 3 + 1] > 185 && row[x * 3 + 2] > 200;
      if (bright && start < 0) start = x;
      if (!bright && start >= 0) { if (x - start > g.card * 0.8) runs.push(x - start); start = -1; }
    }
    assert.equal(runs.length, g.cols, 'Cached original poster stripes must fill every visible grid column');
    assert.ok(runs.every(run => Math.abs(run - g.card) < 8), 'Posters must have the expected density width');
    return runs.length;
  }
  async function peerColors(shot, index, unknown = false) {
    const g = geometry(false);
    const x = Math.round(96 * g.scale + index * (g.card + g.gap));
    const y = Math.round(g.top + g.card * 1.5 + 22 * g.scale + 78 * g.type);
    const rgb = await pixels(shot.path, `${Math.floor(g.card)}x${Math.ceil(24 * g.type)}+${x}+${y}`);
    let green = 0, red = 0;
    for (let i = 0; i < rgb.length; i += 3) {
      if (rgb[i + 1] > 160 && rgb[i] < 150 && rgb[i + 2] < 200) green++;
      if (rgb[i] > 160 && rgb[i + 1] < 150 && rgb[i + 2] < 160) red++;
    }
    if (unknown) assert.equal(green + red, 0, 'Unknown peers must be muted, not green/red');
    else assert.ok(green > 3 && red > 3, 'Known compact peer labels must contain green upload and red download pixels');
  }

  try {
    await start('interaction', true);
    const initial = await shot('1349-comfortable-initial');
    counts(initial);
    assert.match(initial.text, /2\s*releases?/i);
    assert.match(initial.text, /12[.,]0\s*GB/i);
    assert.doesNotMatch(initial.text, /seeders|leechers|Region Free|USA|EUR|1\.100/i,
      'Cards must use compact peer captions and size only, not region/version prose');
    for (const text of initial.passes) {
      assert.ok((text.match(/Peer\s+snapshot/gi) || []).length <= 1, 'Snapshot caption must not repeat per card');
      assert.ok((text.match(/Sep(?:tember)?\s+30,?\s+2026/gi) || []).length <= 1, 'Snapshot date must appear once globally');
    }
    assert.match(initial.text, /snapshot.*Sep(?:tember)?\s+30,?\s+2026.*not liv/i);
    pass('unfiltered grouped counts, two-release card, size-only metadata and a single global snapshot date');

    await toSort();
    await key('Return');
    const menu = await shot('1349-sort-dropdown');
    assert.match(menu.text, /SORT\s+BY/i);
    for (const label of sorts) assert.match(menu.text, label, 'All seven sort options must be visible');
    await key('Down', 'Escape');
    const cancelled = await shot('1349-sort-cancelled');
    assert.doesNotMatch(cancelled.text, /SORT\s+BY/i);
    // OCR skips text enclosed by a bright focus outline. Verify cancellation
    // by the actual game order before choosing any other sort instead.
    await firstHub('sort-cancelled-first-hub', names[0]);
    await toSort();
    await chooseSort(0, 0);
    await firstHub('sort-0-newest-hub', names[0]);
    let currentSort = 0;
    const expectedFirst = [names[0], names[1], names[3], null, names[22], names[1], names[4]];
    for (let next = 1; next < sorts.length; next++) {
      await toSort();
      await chooseSort(currentSort, next);
      currentSort = next;
      const sorted = await shot(`sort-${next}-applied`);
      assert.doesNotMatch(sorted.text, /SORT\s+BY/i);
      counts(sorted);
      if (expectedFirst[next]) await firstHub(`sort-${next}-first-hub`, expectedFirst[next]);
      else await toGrid(); // Ratings intentionally absent; test selection, not fabricated ratings.
    }
    pass('dropdown exposes seven options; Escape cancels; Enter applies; newest/name/date/size/compatibility order verified in safe Hubs');
    await toSort();
    await chooseSort(currentSort, 0);

    // Take density evidence without card focus scaling changing the pixel ruler.
    await key('Right'); // Density idx0 (Comfortable).
    const roomy = await shot('1349-comfortable-density');
    const roomyCols = await visibleColumns(roomy, false);
    await peerColors(roomy, 0);
    await peerColors(roomy, 1, true);
    await peerColors(roomy, 2); // Zero is known: retains green/red, unlike unknown.
    await key('Right', 'Return'); // Density idx1 Compact, persists immediately.
    const compact = await shot('1349-compact-density');
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, true);
    assert.ok(await visibleColumns(compact, true) > roomyCols, 'Compact must show more columns at 1349px');
    counts(compact);
    pass('density changes actual cached poster columns; known/zero peers green/red, unknown muted; compact persisted');

    await key('ctrl+q');
    await stop(active, true);
    await start('compact-restored');
    await key('Up', 'Up', 'Up', 'Right', 'Right'); // Grid -> Search -> Sort -> Density idx0.
    const restored = await shot('1349-compact-restored');
    await visibleColumns(restored, true);
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, true);
    await key('Return'); // Comfortable, while density idx0 focused.
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, false);
    await toGrid();
    await key('Home', 'Down', 'Down'); // Third comfortable row: scrolling away from the first card.
    await shot('1349-scrolled-before-density');
    await key('slash', 'Escape', 'Right', 'Right', 'Right', 'Return');
    // Slash starts search editing without resetting scroll; Escape finishes editing.
    // Then Search -> Sort -> density0 -> density1. Anchor is the first visible row.
    const anchored = await shot('1349-scrolled-compact');
    counts(anchored);
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, true);
    const g0 = geometry(false), g1 = geometry(true);
    const expectedAnchor = Math.floor((2 * g0.cols) / g1.cols) * g1.cols;
    await firstHub('density-scroll-anchor-hub', names[expectedAnchor]);
    await key('slash', 'Escape', 'Right', 'Right', 'Return'); // Comfortable idx0; no Play.
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, false);
    pass('compact survives restart; density preserves a nonzero scroll anchor; original comfortable setting restored');

    await key('Down', 'Down', 'Down', 'Home', 'Up'); // Density -> Grid -> All genres idx4.
    await key('Right', 'Return'); // Action idx5 (four equal genre counts, alphabetical order).
    counts(await shot('filter-action'), 6);
    await key('Up', 'Right', 'Return'); // Status All -> Installed; Action remains selected.
    counts(await shot('filter-installed-action'), 1);
    await key('Down', 'Right', 'Return'); // Action -> Adventure, still Installed.
    const empty = await shot('filter-installed-adventure-empty');
    counts(empty, 0);
    assert.match(empty.text, /No games match your search/i);
    await key('Left', 'Return', 'Up', 'Right', 'Return'); // Action + In-game (any OS) status.
    counts(await shot('filter-ingame-action'), 2);
    await key('Down', 'Left', 'Return'); // All genres; In-game still active.
    counts(await shot('filter-ingame-all-genres'), 3);
    await key('Up', 'Left', 'Left', 'Return'); // Status All, retain All genres.
    counts(await shot('filters-reset'));
    pass('status and genre combine: Action 6, installed Action 1 / Adventure 0, in-game Action 2 / all genres 3');

    await key('Up', 'Return'); // Status -> Search -> edit.
    await exec('xdotool', ['type', '--clearmodifiers', '--delay', '30', 'PPSA98000'], { timeout: 10_000, env });
    counts(await shot('search-title-id-grouped'), 1);
    await key('ctrl+a');
    await exec('xdotool', ['type', '--clearmodifiers', '--delay', '30', 'Moonlit'], { timeout: 10_000, env });
    counts(await shot('search-long-title-two-line-evidence'), 1);
    await key('ctrl+a', 'BackSpace', 'Escape', 'Right', 'Right'); // Clear and restore root focus at Density idx0.

    await resize(960, 640);
    const narrowRoomy = await shot('960-roomy-density');
    counts(narrowRoomy);
    // Review the Roomy label visually; actual density columns are asserted below.
    const narrowCols = await visibleColumns(narrowRoomy, false);
    await key('Right', 'Return');
    const narrowCompact = await shot('960-compact-density');
    counts(narrowCompact);
    assert.ok(await visibleColumns(narrowCompact, true) > narrowCols, 'Compact must show more columns at 960px');
    await key('Left', 'Left', 'Return'); // Density1 -> Density0 -> Sort -> open menu.
    const narrowMenu = await shot('960-sort-dropdown');
    // At this fixed 960x640 test size, recognize the final menu row separately:
    // whole-window segmentation confuses the small "5" with "S". The crop also
    // proves the full caption remains within the actual menu, without clipping.
    const menuCaption = join(artifacts, '960-sort-last-caption.png');
    await exec('convert', [narrowMenu.path, '-crop', '245x22+365+281', '+repage', '-resize', '400%',
      '-colorspace', 'Gray', '-negate', '-normalize', menuCaption], { timeout: 5_000, env });
    const { stdout: menuCaptionText } = await exec('tesseract', [menuCaption, 'stdout', '--psm', '7'], { timeout: 10_000, env });
    await writeFile(join(artifacts, '960-sort-last-caption.txt'), menuCaptionText);
    assert.match(normalize(menuCaptionText), sorts.at(-1));
    narrowMenu.text += ` ${normalize(menuCaptionText)}`;
    for (const label of sorts) assert.match(narrowMenu.text, label);
    await key('Down', 'Escape', 'Right', 'Return'); // Cancel -> Density0 -> Comfortable (label Roomy).
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, false);
    pass('960x640 Roomy/Compact layouts, actual extra columns, all seven dropdown options and original density restoration');

    if (fullHD) {
      await resize(1920, 1080);
      const full = await shot('1920-comfortable-density');
      counts(full);
      await visibleColumns(full, false);
      await key('Right', 'Return');
      await visibleColumns(await shot('1920-compact-density'), true);
      await key('Left', 'Return');
      pass('optional full-HD comfortable/compact evidence');
    }
    await key('ctrl+q');
    await stop(active, true);
    await start('comfortable-restored');
    const final = await shot('comfortable-restored-final');
    counts(final);
    assert.equal(JSON.parse(await readFile(configPath)).library_compact, false);
    // Old fixture config deliberately omits the new option: Settings must show
    // its default-on value. Only our generated profile is ever changed.
    await key('Tab', 's', ...Array(12).fill('Down')); // Home -> Settings -> Seed completed downloads.
    assert.match((await shot('seeding-default-on-settings')).text, /Seed completed downloads/i);
    await key('Left');
    assert.equal(JSON.parse(await readFile(configPath)).seed_after_download, false);
    await shot('seeding-disabled-settings');
    await key('Right');
    assert.equal(JSON.parse(await readFile(configPath)).seed_after_download, true);
    await key('Left', 'Escape');
    await key('ctrl+q');
    await stop(active, true);
    await start('seeding-opt-out-restored');
    assert.equal(JSON.parse(await readFile(configPath)).seed_after_download, false);
    await shot('seeding-opt-out-restored-library');
    await key('ctrl+q');
    await stop(active, true);
    pass('default-on seeding setting can be disabled/enabled; opt-out survives restart without creating a torrent engine');

    const cache = JSON.parse(await readFile(join(cacheDir, 'catalog-rutracker.json')));
    assert.equal(cache.schema, 3);
    assert.equal(cache.games.length, 25);
    assert.equal(cache.snapshot_at, snapshot);
    assert.equal(new Set(cache.games.map(game => game.title_id)).size, 24);
    for (const topic of topics) {
      const game = cache.games.find(game => game.id === Number(topic.id));
      assert.ok(game);
      assert.equal(game.magnet, topic.magnet); // String retention only; never resolve/execute.
      assert.deepEqual(game.game_info, topic.game_info);
      assert.equal(game.seeders, topic.seeders);
      assert.equal(game.leechers, topic.leechers);
    }
    assert.equal(sha256(await readFile(ebootPath)), ebootDigest);
    assert.equal(sha256(await readFile(psnPath)), psnDigest, 'Fresh negative PSN cache must not be enriched');
    assert.equal(sha256(await readFile(compatibilityPath)), compatibilityDigest);
    const cfg = JSON.parse(await readFile(configPath));
    assert.deepEqual(cfg.game_dirs, [installRoot]);
    assert.equal(cfg.emulator, emulator);
    assert.equal(cfg.rawg_key, '');
    for (const absent of [join(configDir, 'transfers/jobs.json'), join(configDir, 'installs/jobs.json'),
      join(base, 'unused-downloads'), join(base, 'unused-installation')]) {
      await assert.rejects(access(absent), { code: 'ENOENT' }, 'Library browsing must not initialize transfers/installations');
    }
    pass('comfortable survives final restart; release metadata retained; inert fixture/caches unchanged; no transfers or installation');
  } finally {
    clearTimeout(deadline);
    process.off('SIGINT', interrupt);
    process.off('SIGTERM', interrupt);
    await stop(active);
    // On failure too, restore ONLY the generated profile's original density bool.
    const cfg = JSON.parse(await readFile(configPath));
    cfg.library_compact = false;
    await writeFile(configPath, JSON.stringify(cfg, null, 2));
    const logs = (await readdir(artifacts)).filter(path => path.endsWith('.log'));
    const combined = (await Promise.all(logs.map(path => readFile(join(artifacts, path), 'utf8')))).join('\n');
    await writeFile(join(artifacts, 'report.json'), JSON.stringify({ binary, checks, images,
      network_namespace: await readlink('/proc/self/ns/net'), original_density_restored: true,
      visual_review: ['Startup frames are best-effort returning-splash/logo evidence, not a forced first-run welcome.',
        'Review white selected pills, two-line long title elision, peer arrows and header spacing in both window sizes.',
        'Fixture header is 24 games / 25 releases; bundled 614-release baseline validated read-only. No real artwork/profile copied.'] }, null, 2));
    assert.doesNotMatch(combined, /panicked at|event loop failed|could not open window|could not save config|Could not save transfer state|detected running game|game started:/i);
  }
  console.log(`Screenshots, OCR, logs and isolated profile retained: ${artifacts}`);
}

try {
  if (inner) await test(); else await outer();
} catch (error) {
  console.error(error.stack || error);
  process.exitCode = 1;
}
#!/usr/bin/env node
// Native Linux smoke tests. All profiles are temporary; the entire test process
// runs in a network namespace, so artwork, updater and torrent traffic cannot occur.
import assert from 'node:assert/strict';
import { spawn, spawnSync, execFile as execFileCallback } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const execFile = promisify(execFileCallback);
const script = fileURLToPath(import.meta.url);
const root = resolve(dirname(script), '..');
const binary = resolve(process.argv[2] || join(root, 'target/release/azrak'));

if (process.env.PS5_CATALOG_TEST_ISOLATED !== '1') {
  const result = spawnSync('unshare', [
    '--user', '--map-root-user', '--net', 'xvfb-run', '-a',
    '-s', '-screen 0 1920x1080x24', process.execPath, script, binary,
  ], {
    stdio: 'inherit', timeout: 180_000,
    env: { ...process.env, PS5_CATALOG_TEST_ISOLATED: '1' },
  });
  if (result.error) console.error(result.error.message);
  if (result.status !== 0) {
    console.error('Requires Linux user/network namespaces, Xvfb, xdotool and ffmpeg. No network-enabled fallback is used.');
  }
  process.exit(result.status ?? 1);
}

const artifacts = await mkdtemp(join(tmpdir(), 'ps5-catalog-smoke-'));
const bundled = join(root, 'assets/rutracker/ps5-topics.json');
const source = JSON.parse(await readFile(bundled, 'utf8'));
const expected = source.topic_count;
assert.equal(source.topics.length, expected);
const broken = join(artifacts, 'invalid.json');
await writeFile(broken, '{invalid JSON');
const peers = join(artifacts, 'peers.json');
await writeFile(peers, JSON.stringify({
  ...source, topic_count: 2,
  topics: [
    { id: '2', title: '[PS5] Unknown peer fixture', game_info: { name: 'Unknown peer fixture' } },
    { id: '1', title: '[PS5] Zero peer fixture', game_info: { name: 'Zero peer fixture' }, seeders: 0, leechers: 0 },
  ],
}));

function waitForHome(child, logs) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => finish(new Error('Home screen did not appear within 20 seconds')), 20_000);
    const check = () => { if (logs().includes('home screen shown')) finish(); };
    const exited = code => finish(new Error(`Launcher exited before Home (code ${code}): ${logs()}`));
    const finish = error => {
      clearTimeout(timer);
      child.stderr.off('data', check);
      child.off('exit', exited);
      if (error) reject(error); else resolve();
    };
    child.stderr.on('data', check);
    child.once('exit', exited);
    check();
  });
}

async function screenshot(name) {
  // Capture a short series of actual X11 frames and keep the final frame, so
  // navigation transitions have rendered before inspecting the screenshot.
  await execFile('ffmpeg', [
    '-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab',
    '-draw_mouse', '0', '-framerate', '10', '-video_size', '1920x1080',
    '-i', process.env.DISPLAY, '-frames:v', '12', '-update', '1',
    join(artifacts, `${name}.png`),
  ], { timeout: 10_000 });
}

async function launch(name, profile, catalog, capture = false) {
  const base = join(artifacts, profile);
  const config = join(base, 'config/azrak');
  await mkdir(config, { recursive: true });
  await writeFile(join(config, 'config.json'), JSON.stringify({
    sounds: false, kyty_auto_update: false, app_auto_update: false, game_dirs: [],
  }));
  const env = {
    ...process.env, XDG_CACHE_HOME: join(base, 'cache'),
    XDG_CONFIG_HOME: join(base, 'config'), XDG_DATA_HOME: join(base, 'data'),
    LIBGL_ALWAYS_SOFTWARE: '1', SLINT_BACKEND: 'winit-femtovg',
  };
  const child = spawn(binary, ['--windowed', '--catalog', catalog, '--sync'], {
    env, stdio: ['ignore', 'ignore', 'pipe'], detached: true,
  });
  let logs = '';
  child.stderr.on('data', bytes => { logs += bytes; });
  const closed = new Promise(resolve => child.once('close', resolve));
  try {
    await waitForHome(child, () => logs);
    const { stdout } = await execFile('xdotool', ['search', '--sync', '--onlyvisible', '--name', '^Azrak Launcher$'], { timeout: 10_000 });
    const window = stdout.trim().split('\n')[0];
    await execFile('xdotool', ['windowfocus', '--sync', window]);
    await execFile('xdotool', ['key', 'Tab']);
    if (capture) {
      await screenshot(`${name}-library`);
      await execFile('xdotool', ['key', 'Return']);
      await screenshot(`${name}-hub`);
    }
    assert.doesNotMatch(logs, /panicked at|event loop failed|could not open window/);
    console.log(`PASS ${name}: native Library opened`);
    return logs;
  } finally {
    if (child.exitCode === null && child.signalCode === null) process.kill(-child.pid, 'SIGTERM');
    await closed;
    await writeFile(join(artifacts, `${name}.log`), logs);
  }
}

const cachePath = join(artifacts, 'retention/cache/azrak/catalog-rutracker.json');
await launch('valid-import', 'retention', bundled, true);
const goodBytes = await readFile(cachePath);
const cache = JSON.parse(goodBytes);
assert.equal(cache.games.length, expected);
assert.equal(cache.schema, 3);
assert.equal(cache.source, source.source);
assert.equal(cache.snapshot_at, source.collected_at);
for (const topic of source.topics) {
  const game = cache.games.find(game => game.id === Number(topic.id));
  assert.ok(game);
  assert.equal(game.magnet, topic.magnet);
  assert.equal(game.seeders, topic.seeders ?? null);
  assert.equal(game.leechers, topic.leechers ?? null);
  assert.deepEqual(game.game_info, topic.game_info);
}
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
for (const [name, path] of [['invalid-retains-cache', broken], ['missing-retains-cache', join(artifacts, 'missing.json')]]) {
  const logs = await launch(name, 'retention', path, name.startsWith('invalid'));
  assert.match(logs, /catalog import failed|catalog source unavailable/);
  assert.equal(digest(await readFile(cachePath)), digest(goodBytes), 'Known-good cache must remain byte-for-byte intact');
}
await launch('invalid-falls-back-to-bundle', 'fresh', broken, true);
await launch('zero-and-unknown', 'peer-profile', peers, true);
const peerCache = JSON.parse(await readFile(join(artifacts, 'peer-profile/cache/azrak/catalog-rutracker.json')));
assert.equal(peerCache.games[0].seeders, null);
assert.equal(peerCache.games[1].seeders, 0);
assert.equal(peerCache.games[0].leechers, null);
assert.equal(peerCache.games[1].leechers, 0);
console.log(`PASS ${expected} topics, magnets and metadata preserved; bad/missing imports retain cache; unknown differs from zero.`);
console.log(`Screenshots and isolated profiles: ${artifacts}`);
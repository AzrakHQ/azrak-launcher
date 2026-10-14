#!/usr/bin/env node
// Native UI smoke: fabricated snapshots and a self-generated game archive. Never start a torrent or run payloads.
// Actual loopback transfer lifecycle is separately exercised by Rust unit tests.
import assert from 'node:assert/strict';
import { spawn, spawnSync, execFile as execCallback } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
const exec = promisify(execCallback);
const script = fileURLToPath(import.meta.url);
const root = resolve(dirname(script), '..');
const binary = resolve(process.argv[2] || join(root, 'target/release/ps5-launcher'));
if (process.env.PS5_DOWNLOAD_TEST_ISOLATED !== '1') {
  const result = spawnSync('unshare', ['--user', '--map-root-user', '--net', 'xvfb-run', '-a',
    '-s', '-screen 0 1920x1080x24', process.execPath, script, binary], {
    stdio: 'inherit', timeout: 90_000, env: { ...process.env, PS5_DOWNLOAD_TEST_ISOLATED: '1' },
  });
  if (result.status !== 0) console.error('Requires unshare/Xvfb/xdotool/ffmpeg/tesseract/ImageMagick; no network-enabled fallback.');
  process.exit(result.status ?? 1);
}
const artifacts = await mkdtemp(join(tmpdir(), 'ps5-download-ui-'));
const config = join(artifacts, 'config/ps5-launcher');
const store = join(config, 'transfers');
const base = join(artifacts, 'downloads');
const installRoot = join(artifacts, 'library');
await mkdir(store, { recursive: true });
await writeFile(join(config, 'config.json'), JSON.stringify({ sounds: false, game_dirs: [],
  downloads_dir: base, games_dir: installRoot, kyty_auto_update: false, app_auto_update: false }));
const hash1 = 'a'.repeat(40), hash2 = 'b'.repeat(40), catalogHash = 'c'.repeat(40);
const folder = join(base, `torrent-${hash1}`);
await mkdir(folder, { recursive: true });
await writeFile(join(folder, '.ps5-launcher-owned'), hash1);
const payload = join(folder, 'generated-ui-fixture.bin');
await writeFile(payload, Buffer.alloc(128 * 1024, 42));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const before = digest(await readFile(payload));
const completed = join(base, `torrent-${catalogHash}`);
const fixture = join(artifacts, 'generated-game');
await mkdir(join(fixture, 'Game/sce_sys'), { recursive: true });
await mkdir(completed, { recursive: true });
await writeFile(join(completed, '.ps5-launcher-owned'), catalogHash);
await writeFile(join(fixture, 'Game/sce_sys/param.json'), JSON.stringify({ titleId: 'PPSA12345', contentVersion: '1.10',
  localizedParameters: { defaultLanguage: 'en-US', 'en-US': { titleName: 'Generated consent fixture' } } }));
const gameBytes = Buffer.alloc(8 * 1024 * 1024, 42); // Non-executable pattern, never launched.
await writeFile(join(fixture, 'Game/eboot.bin'), gameBytes);
const archive = join(completed, 'generated-game.tar');
await exec('tar', ['--format=ustar', '-cf', archive, '-C', fixture, 'Game']);
const archiveBytes = await readFile(archive);
const archiveDigest = digest(archiveBytes);
const jobs = [
  { key: hash1, topic: 10, name: 'Generated progress snapshot', magnet: `magnet:?xt=urn:btih:${hash1}`,
    folder, state: 'Paused', done: 262144, total: 524288, file_count: 1,
    files: ['generated-ui-fixture.bin · 512 KiB'], error: '' },
  { key: hash2, topic: 20, name: 'Generated metadata review', magnet: `magnet:?xt=urn:btih:${hash2}`,
    folder: join(base, `torrent-${hash2}`), state: 'Ready', done: 0, total: 131072, file_count: 2,
    files: ['public-domain-text.txt · 64 KiB', 'generated-pattern.bin · 64 KiB'], error: '' },
  { key: catalogHash, topic: 2, name: 'Generated background installation', magnet: `magnet:?xt=urn:btih:${catalogHash}`,
    folder: completed, state: 'Complete', done: archiveBytes.length, total: archiveBytes.length, file_count: 1,
    files: ['generated-game.tar'], error: '' },
];
const manifest = join(store, 'jobs.json');
await writeFile(manifest, JSON.stringify({ schema: 1, jobs }));
const catalog = JSON.parse(await readFile(join(root, 'assets/rutracker/ps5-topics.json')));
catalog.topics = [
  { id: '2', title: '[PS5] Generated consent fixture', seeders: 22, leechers: 7, game_info: {
    name: 'Generated consent fixture', title_id: 'PPSA12345', version: '1.10', region: 'USA',
    minimum_firmware: '4.00', interface_languages: 'English',
  }, magnet: `magnet:?xt=urn:btih:${catalogHash}` },
  { id: '1', title: '[PS5] Generated consent fixture Deluxe Edition', game_info: {
    name: 'Generated consent fixture Deluxe Edition', title_id: 'PPSA12345', version: '1.00', region: 'EUR',
    minimum_firmware: '4.00', interface_languages: 'English',
  }, magnet: `magnet:?xt=urn:btih:${'d'.repeat(40)}` },
];
catalog.topic_count = 2;
const source = join(artifacts, 'catalog.json');
await writeFile(source, JSON.stringify(catalog));
const child = spawn(binary, ['--windowed', '--catalog', source, '--sync'], {
  detached: true, stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env,
    XDG_CACHE_HOME: join(artifacts, 'cache'), XDG_CONFIG_HOME: join(artifacts, 'config'),
    XDG_DATA_HOME: join(artifacts, 'data'), LIBGL_ALWAYS_SOFTWARE: '1', SLINT_BACKEND: 'winit-femtovg' },
});
let logs = '';
child.stderr.on('data', bytes => { logs += bytes; });
const closed = new Promise(resolve => child.once('close', resolve));
const key = (...keys) => exec('xdotool', ['key', ...keys]);
async function shot(name) {
  const path = join(artifacts, `${name}.png`);
  await exec('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'x11grab',
    '-draw_mouse', '0', '-framerate', '10', '-video_size', '1920x1080', '-i', process.env.DISPLAY,
    '-frames:v', '15', '-update', '1', path], { timeout: 10_000 });
  const { stdout: normal } = await exec('tesseract', [path, 'stdout'], { timeout: 10_000 });
  const { stdout: sparse } = await exec('tesseract', [path, 'stdout', '--psm', '11'], { timeout: 10_000 });
  const contrast = join(artifacts, `${name}-ocr.png`);
  await exec('convert', [path, '-colorspace', 'Gray', '-threshold', '45%', contrast]);
  const { stdout: buttons } = await exec('tesseract', [contrast, 'stdout', '--psm', '11'], { timeout: 10_000 });
  const text = `${normal}\n${sparse}\n${buttons}`;
  await writeFile(join(artifacts, `${name}.txt`), text);
  return text.replace(/\s+/g, ' '); // Wrapped disclosure text is still one readable sentence.
}
try {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => done(new Error('Home did not appear')), 20_000);
    const check = () => { if (logs.includes('home screen shown')) done(); };
    const exit = () => done(new Error(`Launcher exited: ${logs}`));
    function done(error) { clearTimeout(timer); child.stderr.off('data', check); child.off('exit', exit); error ? reject(error) : resolve(); }
    child.stderr.on('data', check); child.once('exit', exit); check();
  });
  const { stdout } = await exec('xdotool', ['search', '--sync', '--onlyvisible', '--name', '^PS5 Launcher$']);
  await exec('xdotool', ['windowfocus', '--sync', stdout.trim().split('\n')[0]]);
  await key('ctrl+d');
  const panel = await shot('transfers');
  assert.match(panel, /Downloads/);
  assert.match(panel, /Generated progress snapshot/);
  assert.match(panel, /50[.,]0%/);
  assert.match(panel, /Paused/);
  assert.match(panel, /Start download/);
  assert.match(panel, /public-domain-text/);
  assert.match(panel, /Open folder/);
  assert.match(panel, /restart never resumes automatically/i);
  await key('Down', 'Right'); // Ready row, Cancel (no engine exists).
  await shot('keyboard-selection');
  await key('Escape', 'Tab');
  const library = await shot('grouped-library');
  assert.match(library, /1\s*games/);
  assert.match(library, /2 releases/);
  await key('Return');
  const hub = await shot('hub');
  // White primary pills are not reliably recognized by OCR; screenshot review covers Install.
  assert.match(hub, /RELEASE VERSION/);
  assert.match(hub, /Release\s*1\s*\/\s*2/i);
  assert.doesNotMatch(hub, /TITLE ID|CONSOLE FIRMWARE NOTE/);
  await key('Down', 'Right'); // Same card, switch exact release/topic/magnet.
  const alternate = await shot('alternate-release');
  assert.match(alternate, /Release\s*2\s*\/\s*2/i);
  assert.match(alternate, /1\.00/);
  await key('Down', 'Return'); // Expand technical rows, not extra overview boxes.
  const details = await shot('technical-details');
  assert.match(details, /TITLE ID/);
  assert.match(details, /PPSA12345/);
  await key('Return', 'Up', 'Up'); // Collapse and return to explicit Download action.
  await key('Return'); // Open confirmation, NOT metadata lookup.
  const consent = await shot('consent');
  assert.match(consent, /Set up a download/);
  assert.match(consent, /Looking up this magnet/);
  assert.match(consent, /128 KiB/i);
  assert.match(consent, /IP address/i);
  // Type spaces/slashes in the destination without triggering the global Enter/Space action.
  await key('Tab', 'ctrl+a'); // Explicit keyboard focus, not OCR-derived click coordinates.
  await exec('xdotool', ['type', '--clearmodifiers', join(artifacts, 'folder with spaces')]);
  const edited = await shot('destination-edit');
  assert.match(edited, /Set up a download/);
  assert.match(edited, /folder with spaces/);
  await key('Escape');
  await shot('consent-cancelled');
  assert.deepEqual(JSON.parse(await readFile(manifest)).jobs.map(job => job.key), [hash1, hash2, catalogHash]);
  await assert.rejects(access(join(store, 'unused-default-output')), 'Consent cancellation must not construct an engine');
  assert.equal(digest(await readFile(payload)), before);
  await key('ctrl+d', 'Right', 'Return'); // Remove paused history, never payload.
  const removed = await shot('history-removed');
  assert.doesNotMatch(removed, /Generated progress snapshot/);
  assert.deepEqual(JSON.parse(await readFile(manifest)).jobs.map(job => job.key), [hash2, catalogHash]);
  assert.equal(digest(await readFile(payload)), before);
  await assert.rejects(access(installRoot), 'Completion alone must never create an installation destination');
  await key('Down', 'Left', 'Return'); // Completed row -> explicit Install confirmation.
  const installConsent = await shot('install-consent');
  assert.match(installConsent, /Install game/);
  assert.match(installConsent, /original downloads are kept/i);
  assert.match(installConsent, /never executes payload/i);
  await assert.rejects(access(installRoot), 'Opening consent must not install');
  await key('Tab', 'ctrl+a');
  const installedRoot = join(artifacts, 'installed library with spaces');
  await exec('xdotool', ['type', '--clearmodifiers', installedRoot]);
  const installEdited = await shot('install-destination-edit');
  assert.match(installEdited, /Install game/);
  assert.match(installEdited, /installed library with spaces/);
  await assert.rejects(access(installedRoot), 'Typing spaces must not activate Install');
  await key('Return'); // Finish editing only; a second explicit Enter is required to Install.
  await assert.rejects(access(join(config, 'installs/jobs.json')), 'Enter while editing must not start installation');
  await key('Escape'); // Cancel without source changes or starting a worker.
  assert.equal(digest(await readFile(archive)), archiveDigest);
  await assert.rejects(access(join(config, 'installs/jobs.json')), 'Cancelled consent must not create an installation record');
  await key('ctrl+d', 'Down', 'Return');
  await shot('install-confirm-again');
  await key('Return'); // Explicit confirmation with original saved destination.
  const installed = await shot('installed');
  assert.match(installed, /Installed/);
  // Review the rendered Play button; filesystem assertions below are the reliable proof.
  const records = JSON.parse(await readFile(join(config, 'installs/jobs.json'))).jobs;
  assert.equal(records.length, 1);
  assert.equal(records[0].state, 'Installed');
  assert.equal(records[0].title_id, 'PPSA12345');
  assert.equal(digest(await readFile(join(records[0].path, 'eboot.bin'))), digest(gameBytes));
  assert.equal(digest(await readFile(archive)), archiveDigest);
  // PS5 games install into the games folder's PS5 folder, which the games folder scan already covers.
  assert.equal(dirname(records[0].path), join(installRoot, 'PS5'));
  assert.deepEqual(JSON.parse(await readFile(join(config, 'config.json'))).game_dirs, []);
  await key('Escape'); // Return to the same Hub, which must now show Play.
  const registeredHub = await shot('registered-hub');
  // Same white primary pill: verify registration text and review the Play screenshot.
  assert.match(registeredHub, /INSTALLED VERSION/);
  assert.doesNotMatch(registeredHub, /Retry install|Installing/);
  await key('ctrl+q');
  await Promise.race([closed, new Promise((_, reject) => { const t = setTimeout(() => reject(new Error('Graceful quit timed out')), 10_000); t.unref(); })]);
  assert.equal(child.exitCode, 0);
  assert.doesNotMatch(logs, /panicked at|event loop failed|Could not save transfer state/);
  console.log('PASS grouped cards/releases/compact hub, progress, editable download/install consent, explicit archive installation/library registration/Play, retained sources and graceful quit.');
  console.log(`Screenshots and isolated profiles: ${artifacts}`);
} finally {
  if (child.exitCode === null && child.signalCode === null) process.kill(-child.pid, 'SIGTERM');
  await closed;
  await writeFile(join(artifacts, 'launcher.log'), logs);
}
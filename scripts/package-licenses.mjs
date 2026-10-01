#!/usr/bin/env node
// Node 22+, standard library only. Run after the locked Linux release build.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../', import.meta.url));
// Linux unless the release job names another target (the macOS bundle passes aarch64-apple-darwin).
export const TARGET = process.env.PS5_LICENSE_TARGET || 'x86_64-unknown-linux-gnu';
const ROYALTY = 'LicenseRef-Slint-Royalty-free-2.0';
const RQBIT_COMMIT = 'a499d2f243d124e144aef137afe7cb304a6e3f36';
const SLINT_COMMIT = '372cf0ee5577c3dfec309a45e7b778ba4e81b734';
const FALLBACK_REPOS = new Set([
  'rodrimati1992/assert_cfg', 'slint-ui/slint', 'brendanzab/gl-rs',
  'boinkor-net/governor', 'ikatson/rqbit', 'nical/lyon',
  'mondeja/rspolib', 'DioxusLabs/taffy',
]);
// Reviewed repositories that moved since their crates were published. GitHub redirects the
// old name, but requests never follow redirects, so ask the new location directly.
const MOVED_REPOS = new Map([['brendanzab/gl-rs', 'rust-windowing/gl-rs']]);
const DOCUMENT = /^(?:licen[cs]es?|notices?|copying|copyright|authors|ofl)(?:[._-]|$)/i;
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const key = p => `${p.name}@${p.version}`;

export function safeRelative(value) {
  if (!value || path.posix.isAbsolute(value) || value.includes('\\') ||
      value.split('/').some(p => !p || p === '.' || p === '..')) {
    throw new Error('Unsafe document path');
  }
  return value;
}

export function normalizeExpression(expression) {
  if (!expression) throw new Error('Dependency has no declared license');
  // Preserve Cargo's original field separately; old slash syntax means OR.
  return expression.replace(/\s*\/\s*/g, ' OR ');
}

export function selectPackages(metadata, tree) {
  const names = new Set();
  for (const line of tree.trim().split('\n')) {
    const match = line.match(/^(\S+) v(\S+)(?: |$)/);
    if (!match) throw new Error('Unrecognized cargo tree output');
    names.add(`${match[1]}@${match[2]}`);
  }
  const packages = metadata.packages.filter(p => names.has(key(p)));
  if (packages.length !== names.size) throw new Error('Ambiguous or missing cargo tree package identity');
  return packages.filter(p => p.id !== metadata.resolve.root)
    .sort((a, b) => key(a).localeCompare(key(b), 'en'));
}

// Walk every directory, including vendored native code. Never follow a directory
// symlink; document file symlinks must resolve inside the source package.
export async function localDocuments(root, licenseFile = null, allFiles = false) {
  const base = await fs.realpath(root);
  const documents = [];
  async function visit(directory, inherited = false) {
    for (const entry of (await fs.readdir(directory, { withFileTypes: true }))
      .sort((a, b) => a.name.localeCompare(b.name, 'en'))) {
      const absolute = path.join(directory, entry.name);
      const relative = path.relative(base, absolute).split(path.sep).join('/');
      const matched = inherited || DOCUMENT.test(entry.name);
      if (entry.isDirectory()) await visit(absolute, matched);
      else if (allFiles || matched || relative === licenseFile) {
        const real = await fs.realpath(absolute);
        if (!real.startsWith(`${base}${path.sep}`)) throw new Error('Document symlink escapes package');
        if (!(await fs.stat(real)).isFile()) throw new Error('Document is not a regular file');
        const bytes = await fs.readFile(real);
        if (!bytes.length) throw new Error(`Empty document: ${safeRelative(relative)}`);
        documents.push({ relative: safeRelative(relative), bytes });
      }
    }
  }
  if (licenseFile) safeRelative(licenseFile);
  await visit(base);
  if (licenseFile && !documents.some(d => d.relative === licenseFile)) {
    throw new Error('Declared license_file is absent');
  }
  return documents;
}

export function applicableUpstreamFiles(tree, packagePath, expression, repo) {
  const ancestors = new Set(['']);
  for (let dir = packagePath; dir; dir = path.posix.dirname(dir)) {
    if (dir === '.') break;
    ancestors.add(dir);
  }
  const files = tree.filter(e => e.type === 'blob' && DOCUMENT.test(path.posix.basename(e.path)) &&
    (ancestors.has(path.posix.dirname(e.path) === '.' ? '' : path.posix.dirname(e.path)) ||
      (packagePath && e.path.startsWith(`${packagePath}/`))));
  if (repo === 'slint-ui/slint') {
    // REUSE license texts live at repository root, not in published .crate files.
    const licenses = expression.includes(ROYALTY) ? [ROYALTY] : ['MIT', 'Apache-2.0'];
    for (const license of licenses) {
      const entry = tree.find(e => e.path === `LICENSES/${license}.${license === ROYALTY ? 'md' : 'txt'}`);
      if (!entry) throw new Error(`Missing upstream ${license} text`);
      files.push(entry);
    }
    // Do not copy unchosen commercial/GPL texts through package symlinks.
    return files.filter(e => !e.path.includes('/LICENSES/') || licenses.some(l =>
      path.posix.basename(e.path).startsWith(`${l}.`)));
  }
  return files;
}

async function upstreamDocuments(pkg, allowUpstream, cache) {
  if (!allowUpstream) throw new Error(`${key(pkg)} needs upstream documents; enable --allow-upstream`);
  const vcs = JSON.parse(await fs.readFile(path.join(path.dirname(pkg.manifest_path), '.cargo_vcs_info.json'), 'utf8'));
  const commit = vcs.git?.sha1;
  if (!/^[a-f0-9]{40}$/.test(commit ?? '')) throw new Error(`${key(pkg)} lacks a published VCS commit`);
  // Two rqbit workspace crates omit repository metadata. Limit this override to
  // the exact audited publication, never guess a repository for another crate.
  const rqbit = pkg.name.startsWith('librqbit') && pkg.version === '9.0.1' && commit === RQBIT_COMMIT;
  const repository = pkg.repository ?? (rqbit ? 'https://github.com/ikatson/rqbit' : '');
  const match = repository.match(/^https:\/\/github\.com\/([^/]+\/[^/#]+)(?:\/|$)/);
  const repo = match?.[1].replace(/\.git$/, '');
  if (!FALLBACK_REPOS.has(repo)) throw new Error(`${key(pkg)} has no reviewed upstream fallback`);
  if (pkg.license.includes(ROYALTY) && (pkg.version !== '1.18.1' || commit !== SLINT_COMMIT)) {
    throw new Error('New Slint publication requires attribution/license review');
  }
  const packagePath = vcs.path_in_vcs ?? '';
  if (packagePath) safeRelative(packagePath);
  const location = MOVED_REPOS.get(repo) ?? repo;
  const base = `${location}/${commit}`;
  async function request(url, json = false) {
    if (!cache.has(url)) cache.set(url, (async () => {
      const headers = { Accept: json ? 'application/vnd.github+json' : 'text/plain' };
      // In CI, the workflow token avoids the shared anonymous API rate limit. Never logged.
      if (json && process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
      const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(30000), headers });
      if (!response.ok) throw new Error(`Official dependency source returned HTTP ${response.status}`);
      return json ? response.json() : Buffer.from(await response.arrayBuffer());
    })());
    return cache.get(url);
  }
  const treeUrl = `https://api.github.com/repos/${location}/git/trees/${commit}?recursive=1`;
  const listing = await request(treeUrl, true);
  if (listing.truncated || !Array.isArray(listing.tree)) throw new Error('Incomplete upstream notice listing');
  const entries = applicableUpstreamFiles(listing.tree, packagePath, pkg.license, repo);
  if (!entries.some(e => /^(?:licen[cs]e|copying)/i.test(path.posix.basename(e.path)))) {
    throw new Error(`${key(pkg)} has no upstream LICENSE text`);
  }
  async function content(entry, seen = new Set()) {
    safeRelative(entry.path);
    if (seen.has(entry.path)) throw new Error('Cyclic upstream license symlink');
    seen.add(entry.path);
    const url = `https://raw.githubusercontent.com/${base}/${entry.path}`;
    const bytes = await request(url);
    if (entry.mode === '120000') {
      const target = safeRelative(path.posix.normalize(path.posix.join(path.posix.dirname(entry.path), bytes.toString('utf8').trim())));
      const destination = listing.tree.find(e => e.path === target && e.type === 'blob');
      if (!destination) throw new Error('Broken upstream license symlink');
      return content(destination, seen);
    }
    if (!bytes.length || /^\s*(?:<!doctype|<html)/i.test(bytes.toString('utf8'))) {
      throw new Error('Upstream license response is empty or an HTML page');
    }
    return { bytes, source: url };
  }
  const documents = [];
  for (const entry of entries.sort((a, b) => a.path.localeCompare(b.path, 'en'))) {
    documents.push({ relative: `upstream/${safeRelative(entry.path)}`, ...await content(entry) });
  }
  return { documents, upstreamNoticeSearch: {
    source: treeUrl, packagePath, scope: 'Repository root, package ancestors and package subtree',
    noticePaths: entries.filter(e => /^notices?(?:[._-]|$)/i.test(path.posix.basename(e.path))).map(e => e.path),
  } };
}

export async function packageLicenses({ output, cargo = process.env.CARGO || 'cargo', allowUpstream = false }) {
  const releaseRoot = await fs.realpath(output);
  const destination = path.join(releaseRoot, 'licenses');
  const docDestination = path.join(releaseRoot, 'THIRD-PARTY-NOTICES.md');
  for (const file of [destination, docDestination]) {
    try { await fs.lstat(file); throw new Error('Refusing to overwrite existing notice output'); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  const run = args => execFileSync(cargo, args, { cwd: ROOT, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'] });
  const metadata = JSON.parse(run(['metadata', '--locked', '--offline', '--filter-platform', TARGET, '--format-version', '1']));
  const packages = selectPackages(metadata, run(['tree', '--locked', '--offline', '--target', TARGET,
    '--edges', 'normal,build', '--prefix', 'none', '--format', '{p}']));
  if (!packages.some(p => p.name === 'slint' && p.version === '1.18.1') ||
      !packages.some(p => p.name === 'librqbit' && p.version === '9.0.1')) {
    throw new Error('Expected audited Slint 1.18.1 and librqbit 9.0.1 dependencies are absent');
  }
  const stage = await fs.mkdtemp(path.join(releaseRoot, '.license-stage-'));
  const cache = new Map();
  const inventory = { schemaVersion: 1, target: TARGET,
    scope: 'Active default-feature normal/build crates; build-time crates conservatively retained; no dev-only or unused optional crates',
    certification: false, cargoLockSha256: sha256(await fs.readFile(path.join(ROOT, 'Cargo.lock'))),
    packages: [], assets: [], supplementalLicenses: [], reviewNotes: [
      'This SPDX-expression inventory is not an SPDX SBOM or comprehensive legal certification.',
      'Slint section 2 requires AboutSlint in an accessible About screen/splash screen OR the official badge on an easily found public webpage. Archive notices alone do not satisfy attribution.',
      'System libraries, toolchain/runtime components, generated code and embedded assets beyond the retained font notices need separate review.',
      'The application license is not set or changed by this inventory.',
    ] };
  async function store(relative, bytes, source) {
    safeRelative(relative);
    const file = path.join(stage, relative);
    await fs.mkdir(path.dirname(file), { recursive: true });
    await fs.writeFile(file, bytes, { flag: 'wx' });
    return { path: `licenses/${relative}`, sha256: sha256(bytes), source };
  }
  let apache = null;
  try {
    for (const pkg of packages) {
      const expression = normalizeExpression(pkg.license);
      const record = { name: pkg.name, version: pkg.version, declaredLicense: pkg.license,
        spdxExpression: expression, selectedLicense: expression.includes(ROYALTY) ? ROYALTY : null,
        repository: pkg.repository, authors: pkg.authors ?? [],
        source: `https://crates.io/crates/${pkg.name}/${pkg.version}`, documents: [] };
      const documents = await localDocuments(path.dirname(pkg.manifest_path), pkg.license_file);
      for (const document of documents) {
        const source = `crate:${key(pkg)}/${document.relative}`;
        record.documents.push(await store(`crates/${key(pkg)}/${document.relative}`, document.bytes, source));
        const text = document.bytes.toString('utf8');
        if (!apache && /^\s*Apache License\s+Version 2\.0/.test(text) &&
            text.includes('END OF TERMS AND CONDITIONS') && text.length > 9000) {
          apache = { bytes: document.bytes, source };
        }
      }
      if (!documents.some(d => /^(?:licen[cs]e|copying)/i.test(path.posix.basename(d.relative))) || expression.includes(ROYALTY)) {
        const fallback = await upstreamDocuments(pkg, allowUpstream, cache);
        for (const document of fallback.documents) record.documents.push(await store(
          `crates/${key(pkg)}/${document.relative}`, document.bytes, document.source));
        record.upstreamNoticeSearch = fallback.upstreamNoticeSearch;
        if (!pkg.repository && pkg.name.startsWith('librqbit')) record.repository = 'https://github.com/ikatson/rqbit';
      }
      if (expression === 'MPL-2.0') {
        // Supply the exact cached Covered Software source, not the app's source.
        record.coveredSource = [];
        for (const document of await localDocuments(path.dirname(pkg.manifest_path), null, true)) {
          record.coveredSource.push(await store(`crates/${key(pkg)}/source/${document.relative}`,
            document.bytes, `crate:${key(pkg)}/${document.relative}`));
        }
      }
      inventory.packages.push(record);
    }
    if (!apache) throw new Error('No complete Apache-2.0 standard text found in selected local crates');
    inventory.supplementalLicenses.push(await store('standard/Apache-2.0.txt', apache.bytes, apache.source));
    for (const pkg of inventory.packages.filter(p => p.spdxExpression === 'Apache-2.0')) {
      pkg.standardLicense = 'licenses/standard/Apache-2.0.txt';
    }
    for (const document of await localDocuments(path.join(ROOT, 'assets/fonts'))) {
      inventory.assets.push(await store(`assets/fonts/${document.relative}`, document.bytes,
        `assets/fonts/${document.relative}`));
    }
    await fs.writeFile(path.join(stage, 'inventory.json'), `${JSON.stringify(inventory, null, 2)}\n`, { flag: 'wx' });
    await fs.copyFile(path.join(ROOT, 'docs/third-party-notices.md'), docDestination, fs.constants.COPYFILE_EXCL);
    await fs.rename(stage, destination);
    console.log(`Packaged notices for ${inventory.packages.length} crates (${inventory.packages.filter(p => p.upstreamNoticeSearch).length} upstream fallbacks).`);
    console.log('REVIEW REQUIRED: Slint visible widget OR public webpage badge; this archive is not legal certification.');
    return inventory;
  } catch (error) {
    await fs.rm(stage, { recursive: true, force: true });
    throw error;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (!args[0] || args.some((arg, i) => i > 0 && arg !== '--allow-upstream')) {
    console.error('Usage: node scripts/package-licenses.mjs RELEASE_DIRECTORY [--allow-upstream]');
    process.exitCode = 1;
  } else {
    packageLicenses({ output: args[0], allowUpstream: args.includes('--allow-upstream') }).catch(error => {
      // Do not print Cargo stderr, credentials, environment or complete metadata.
      console.error(`License packaging failed: ${error.status !== undefined ? 'Cargo failed (locked/offline dependency cache required)' : error.message}`);
      process.exitCode = 1;
    });
  }
}
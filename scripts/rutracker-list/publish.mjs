#!/usr/bin/env node
// Package the generated public metadata snapshot as an offline launcher asset.
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { platformFrom } from './platforms.mjs';

const platform = platformFrom(process.argv.slice(2));
const input = fileURLToPath(new URL(`../../dist/rutracker/${platform.key}-topics.json`, import.meta.url));
const output = fileURLToPath(new URL(`../../assets/rutracker/${platform.key}-topics.json`, import.meta.url));
const bytes = await readFile(input);
const report = JSON.parse(bytes);
if (!report.complete || report.translation?.language !== 'en'
    || !Array.isArray(report.topics) || !report.topics.length
    || report.topic_count !== report.topics.length
    || new Set(report.topics.map(topic => topic.id)).size !== report.topics.length
    || report.source !== platform.source) {
  throw new Error(`Only a complete, English, deduplicated ${platform.label} forum snapshot can be bundled`);
}
await mkdir(dirname(output), { recursive: true });
await writeFile(`${output}.tmp`, bytes);
await rename(`${output}.tmp`, output);
console.log(`Bundled ${report.topics.length} release topics; no network requests or downloads.`);
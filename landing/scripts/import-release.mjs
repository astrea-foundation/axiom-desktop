import {mkdir, readFile, writeFile, rename} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
import {compareVersions, parseRelease} from '../../packages/desktop-releases/manifest.mjs';

// Run after the CDN publisher has uploaded and verified every artifact.
const source = process.argv[2];
if (!source) throw new Error('Usage: node landing/scripts/import-release.mjs /path/to/manifest.json');
const root = fileURLToPath(new URL('..', import.meta.url));
const release = parseRelease(JSON.parse(await readFile(source, 'utf8')));
const current = parseRelease(JSON.parse(await readFile(path.join(root, 'releases.json'), 'utf8')));
if (compareVersions(release.version, current.version) < 0) throw new Error('Cannot promote an older release');
await mkdir(path.join(root, 'releases'), {recursive: true});
for (const item of [current, release]) {
  const file = path.join(root, 'releases', `${item.version}.json`);
  const text = JSON.stringify(item, null, 2) + '\n';
  try { await writeFile(file, text, {flag: 'wx'}); }
  catch (error) {
    if (error.code !== 'EEXIST') throw error;
    if (JSON.stringify(parseRelease(JSON.parse(await readFile(file, 'utf8')))) !== JSON.stringify(item)) throw new Error(`Release ${item.version} is immutable`);
  }
}
const next = path.join(root, 'releases.json.next');
await writeFile(next, JSON.stringify(release, null, 2) + '\n', {flag: 'wx'});
await rename(next, path.join(root, 'releases.json'));
console.log(`Prepared ${release.version}. Build and deploy the website to publish its API and download page together.`);

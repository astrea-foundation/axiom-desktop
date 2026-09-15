import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readdir, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import {parseRelease} from '../packages/desktop-releases/manifest.mjs';

const [directory, version, signing = 'unsigned'] = process.argv.slice(2);
if (!directory || !/^\d+\.\d+\.\d+$/.test(version ?? '') || !['unsigned','signed'].includes(signing)) {
  throw new Error('Usage: download-manifest.mjs DIRECTORY VERSION [unsigned|signed]');
}
const downloads = [];
for (const name of (await readdir(directory)).sort()) {
  if (!/^[A-Za-z0-9._-]+\.(?:exe|dmg|pkg|zip|AppImage|deb|pkg\.tar\.(?:xz|zst))$/.test(name)) continue;
  if (!name.includes(`-${version}-`)) throw new Error(`Artifact version mismatch: ${name}`);
  const file = path.join(directory, name);
  const metadata = await stat(file);
  if (!metadata.isFile() || metadata.size === 0) throw new Error(`Invalid artifact: ${name}`);
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  const sha256 = hash.digest('hex');
  const platform = /\.(?:dmg|pkg|zip)$/.test(name) ? 'mac' : name.endsWith('.exe') ? 'win' : 'linux';
  const arch = /-arm64[.-]/.test(name) ? 'arm64' : /-(?:x64|x86_64|amd64)[.-]/.test(name) ? 'x64' : null;
  if (!arch) throw new Error(`Unknown architecture: ${name}`);
  downloads.push({ name, platform, arch, bytes: metadata.size, sha256, url: `https://cdn.axiom.stream/downloads/${version}/${name}` });
}
if (downloads.length === 0) throw new Error('No installer artifacts found');
await writeFile(path.join(directory, 'SHA256SUMS'), downloads.map(d => `${d.sha256}  ${d.name}\n`).join(''));
await writeFile(path.join(directory, 'manifest.json'), JSON.stringify(parseRelease({ version, signing, downloads }), null, 2) + '\n');
console.log(`Prepared manifest and checksums for ${downloads.length} installers.`);

import {createHash} from 'node:crypto';
import {createReadStream} from 'node:fs';
import {lstat, readFile, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {CLI_FORMATS, FORMATS, parseRelease} from '../packages/desktop-releases/manifest.mjs';

export async function verifyCandidate(directory, {platform, arch, revision, version}) {
  if (!Object.hasOwn(FORMATS, platform) || !['x64', 'arm64'].includes(arch)
      || (platform === 'linux' && arch !== 'x64')) throw new Error('Unsupported candidate target');
  const manifest = parseRelease(JSON.parse(await readFile(path.join(directory, 'manifest.json'), 'utf8')));
  if (manifest.signing !== 'unsigned' || manifest.revision !== revision || manifest.version !== version) {
    throw new Error('Candidate must be unsigned and match the selected source revision and existing version');
  }
  const expected = new Set([
    ...FORMATS[platform].map(format => `desktop:${format}`),
    ...CLI_FORMATS[platform].map(format => `cli:${format}`),
  ]);
  const files = [...manifest.downloads, ...manifest.cliDownloads];
  for (const file of files) {
    if (file.platform !== platform || file.arch !== arch || !expected.delete(`${file.product}:${file.format}`)) {
      throw new Error('Unexpected candidate artifact target');
    }
    const filename = path.join(directory, file.name);
    const metadata = await lstat(filename);
    if (!metadata.isFile() || metadata.size !== file.bytes) throw new Error(`Candidate artifact size/type mismatch: ${file.name}`);
    const hash = createHash('sha256');
    for await (const chunk of createReadStream(filename)) hash.update(chunk);
    if (hash.digest('hex') !== file.sha256) throw new Error(`Candidate artifact hash mismatch: ${file.name}`);
  }
  if (expected.size) throw new Error(`Incomplete candidate: ${[...expected].join(', ')}`);
  const checksums = files.map(file => `${file.sha256}  ${file.name}\n`).join('');
  if (await readFile(path.join(directory, 'SHA256SUMS'), 'utf8') !== checksums) throw new Error('Candidate checksum inventory mismatch');
  return {schemaVersion: 1, revision, version, platform, arch, signing: 'unsigned', files: files.map(({name, bytes, sha256}) => ({name, bytes, sha256}))};
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const [directory, platform, arch, revision, version, ...extra] = process.argv.slice(2);
  if (!directory || !version || extra.length) throw new Error('Usage: verify-candidate.mjs DIRECTORY linux|mac|win x64|arm64 COMMIT_SHA VERSION');
  const receipt = await verifyCandidate(directory, {platform, arch, revision, version});
  await writeFile(path.join(directory, 'candidate-validation.json'), JSON.stringify(receipt, null, 2) + '\n');
  console.log(`Validated ${receipt.files.length} unsigned ${platform}/${arch} candidate installers at ${revision}.`);
}

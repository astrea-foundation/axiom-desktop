import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {CLI_FORMATS, FORMATS} from '../packages/desktop-releases/manifest.mjs';
import {verifyCandidate} from './verify-candidate.mjs';

async function fixture(t, platform = 'linux', arch = 'x64') {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'axiom-candidate-'));
  t.after(() => rm(directory, {recursive: true, force: true}));
  const options = {platform, arch, revision: 'a'.repeat(40), version: '0.1.21'};
  const manifest = {schemaVersion: 2, version: options.version, revision: options.revision, sequence: 1, signing: 'unsigned', downloads: [], cliDownloads: []};
  for (const [product, formats] of [['desktop', FORMATS[platform]], ['cli', CLI_FORMATS[platform]]]) {
    for (const format of formats) {
      const suffix = format === 'pacman' ? 'pkg.tar.xz' : format;
      const name = `${product === 'cli' ? 'AxiomCLI' : 'Axiom'}-${options.version}-${platform}-${arch}.${suffix}`;
      const bytes = Buffer.from(`unsigned fixture: ${name}`);
      await writeFile(path.join(directory, name), bytes);
      manifest[product === 'cli' ? 'cliDownloads' : 'downloads'].push({name, product, platform, arch, format, bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'), url: `https://cdn.axiom.stream/downloads/${options.version}/${name}`,
        githubUrl: `https://github.com/astrea-foundation/axiom-releases/releases/download/v${options.version}/${name}`});
    }
  }
  async function stamp() {
    await writeFile(path.join(directory, 'manifest.json'), JSON.stringify(manifest));
    await writeFile(path.join(directory, 'SHA256SUMS'), [...manifest.downloads, ...manifest.cliDownloads].map(file => `${file.sha256}  ${file.name}\n`).join(''));
  }
  await stamp();
  return {directory, options, manifest, stamp};
}

test('validates all required Desktop and CLI formats for each native target', async t => {
  for (const [platform, arch] of [['linux', 'x64'], ['mac', 'x64'], ['mac', 'arm64'], ['win', 'x64'], ['win', 'arm64']]) {
    const {directory, options} = await fixture(t, platform, arch);
    const receipt = await verifyCandidate(directory, options);
    assert.equal(receipt.files.length, platform === 'linux' ? 4 : 2);
    assert.equal(receipt.revision, options.revision);
    assert.equal(receipt.signing, 'unsigned');
  }
});

test('rejects partial packages, stale source/version and other target inventories', async t => {
  const {directory, options, manifest, stamp} = await fixture(t);
  for (const replacement of [{revision: 'b'.repeat(40)}, {version: '0.1.22'}, {platform: 'mac'}, {arch: 'arm64'}]) {
    await assert.rejects(verifyCandidate(directory, {...options, ...replacement}));
  }
  manifest.cliDownloads = [];
  await stamp();
  await assert.rejects(verifyCandidate(directory, options), /Incomplete candidate/);
});

test('rejects modified artifact bytes even when the length is unchanged', async t => {
  const {directory, options, manifest} = await fixture(t);
  const file = path.join(directory, manifest.downloads[0].name);
  const bytes = await readFile(file);
  bytes[0] ^= 1;
  await writeFile(file, bytes);
  await assert.rejects(verifyCandidate(directory, options), /hash mismatch/);
});

test('rejects missing, truncated and mismatched checksum artifacts', async t => {
  const {directory, options, manifest} = await fixture(t);
  await writeFile(path.join(directory, 'SHA256SUMS'), 'unrelated inventory\n');
  await assert.rejects(verifyCandidate(directory, options), /checksum inventory/);
  await writeFile(path.join(directory, manifest.downloads[0].name), 'truncated');
  await assert.rejects(verifyCandidate(directory, options), /size\/type mismatch/);
  await rm(path.join(directory, manifest.downloads[0].name));
  await assert.rejects(verifyCandidate(directory, options), {code: 'ENOENT'});
});

test('rejects signed release metadata in the unsigned candidate path', async t => {
  const {directory, options, manifest, stamp} = await fixture(t);
  manifest.signing = 'signed';
  manifest.signature = {keyId: 'a'.repeat(16), value: 'A'.repeat(86) + '=='};
  await stamp();
  await assert.rejects(verifyCandidate(directory, options), /Candidate must be unsigned/);
});

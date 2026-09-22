import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseRelease } from '../../../packages/desktop-releases/manifest.mjs';

const script = fileURLToPath(new URL('../../../scripts/download-manifest.mjs', import.meta.url));
test('combined release generator keeps CLI installers separate and emits installable checksums', () => {
  const dir = mkdtempSync(join(tmpdir(), 'axiom-manifest-'));
  try {
    const desktop = 'Axiom-0.1.3-linux-x64.AppImage';
    const cli = 'AxiomCLI-0.1.3-linux-x64.sh';
    writeFileSync(join(dir, desktop), 'desktop fixture');
    writeFileSync(join(dir, cli), 'cli fixture');
    const result = spawnSync(process.execPath, [script, dir, '0.1.3'], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    const release = parseRelease(JSON.parse(readFileSync(join(dir, 'manifest.json'), 'utf8')));
    assert.deepEqual(release.downloads.map(file => file.name), [desktop]);
    assert.deepEqual(release.cliDownloads?.map(file => file.name), [cli]);
    const checksum = `${createHash('sha256').update('cli fixture').digest('hex')}  ${cli}\n`;
    assert.ok(readFileSync(join(dir, 'SHA256SUMS'), 'utf8').includes(checksum));
    // Invalid installers must not rewrite metadata.
    const accepted = readFileSync(join(dir, 'manifest.json'), 'utf8');
    writeFileSync(join(dir, 'AxiomCLI-0.1.3-linux-wrong.sh'), 'wrong architecture');
    assert.notEqual(spawnSync(process.execPath, [script, dir, '0.1.3']).status, 0);
    assert.equal(readFileSync(join(dir, 'manifest.json'), 'utf8'), accepted);
  } finally { rmSync(dir, { force: true, recursive: true }); }
});

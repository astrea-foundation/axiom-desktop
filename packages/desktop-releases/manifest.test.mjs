import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseRelease } from './manifest.mjs';

const contract = JSON.parse(readFileSync(new URL('./contract-fixtures.json', import.meta.url), 'utf8'));
for (const fixture of contract.cases) {
  test(`release contract: ${fixture.name}`, () => {
    const value = structuredClone(contract.release);
    for (const change of fixture.changes) {
      const parts = change.path.slice(1).split('/');
      const key = parts.pop();
      const parent = parts.reduce((value, part) => value[part], value);
      if (change.remove) delete parent[key];
      else if (change.repeat) parent[key] = Array.from({length: change.repeat}, () => structuredClone(parent[key][0]));
      else parent[key] = change.value;
    }
    if (fixture.valid) {
      const parsed = parseRelease(value);
      assert.equal(parsed.version, value.version);
      assert.deepEqual(parseRelease(parsed), parsed);
      assert.ok(parsed.downloads.every(file => file.format !== 'tar.gz'));
      assert.equal(parsed.cliDownloads?.length ?? 0, value.cliDownloads?.length ?? 0);
    } else {
      assert.throws(() => parseRelease(value));
    }
  });
}

const {generateKeyPairSync} = await import('node:crypto');
const {publicKeyHex, signRelease, verifyRelease} = await import('./signing.mjs');
const {requireCompleteRelease} = await import('./manifest.mjs');
const signed = JSON.parse(readFileSync(new URL('./signed-fixture.json',import.meta.url),'utf8'));
const universal = JSON.parse(readFileSync(new URL('./universal-fixture.json',import.meta.url),'utf8'));
test('combined release has eight signed installers and rejects mixed architecture inventories', () => {
  const {release,publicKey} = universal;
  verifyRelease(release,publicKey);
  requireCompleteRelease(release);
  assert.equal(release.downloads.length,5);
  assert.equal(release.cliDownloads.length,3);
  for (const mutate of [r=>r.cliDownloads.pop(),r=>r.schemaVersion=2,r=>r.downloads[0].arch='x64']) {
    const changed=structuredClone(release);mutate(changed);
    assert.throws(()=>requireCompleteRelease(parseRelease(changed)));
  }
});
test('Node signature matches the native shared vector and rejects altered metadata or keys', () => {
  verifyRelease(signed.release,signed.publicKey);
  for (const mutate of [r=>r.downloads[0].sha256='f'.repeat(64),r=>r.sequence++,r=>r.revision='b'.repeat(40),r=>r.cliDownloads.pop()]) {
    const release=structuredClone(signed.release);mutate(release);
    assert.throws(()=>verifyRelease(release,signed.publicKey));
  }
  assert.throws(()=>verifyRelease(signed.release,'c'.repeat(64)));
  assert.throws(()=>verifyRelease(contract.release,signed.publicKey));
});
test('signing enforces the matching Ed25519 trust identity and complete publication matrix', () => {
  const {privateKey}=generateKeyPairSync('ed25519');
  const pem=privateKey.export({type:'pkcs8',format:'pem'}), key=publicKeyHex(pem);
  const release=signRelease(contract.release,pem,key);
  verifyRelease(release,key);requireCompleteRelease(release);
  assert.throws(()=>signRelease(contract.release,pem,signed.publicKey));
  release.cliDownloads.pop();assert.throws(()=>requireCompleteRelease(release),/Incomplete/);
});

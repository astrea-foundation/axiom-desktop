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

import assert from 'node:assert/strict';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { AxiomAcpClient } from '../src/client.js';

const fixture = fileURLToPath(new URL('./fixtures/version-sidecar.mjs', import.meta.url));
for (const version of ['0.1.2', '0.1.1', '0.1.3', 'missing', '0.1.2-beta']) {
  test(`desktop product handshake checks ${version} before exposing connected state`, async () => {
    const client = new AxiomAcpClient({ command: process.execPath, args: [fixture, version] });
    const connected: boolean[] = [];
    client.on('state', state => connected.push(state.connected));
    try {
      if (version === '0.1.2') {
        await client.initialize('0.1.2');
        assert.equal(client.getState().connected, true);
      } else {
        await assert.rejects(client.initialize('0.1.2'), /requires AxiomCLI 0.1.2/);
        assert.equal(client.getState().connected, false);
        assert.ok(connected.every(value => !value));
        await assert.rejects(client.bootstrapDesktop(), /not initialized/);
      }
    } finally { await client.close(); }
  });
}

import assert from 'node:assert/strict';
import test from 'node:test';
// @ts-expect-error Release scripts run directly in Node without generated declarations.
import { installedEnvironment, verifiedImageUsage } from '../../../scripts/verify-installed-image.mjs';

test('signed installation acceptance uses isolated profiles and drops provider overrides', () => {
  const source = { HOME: '/home/operator', PATH: '/usr/bin', AXIOM_API_KEY: 'automation-test',
    AXIOM_DIRECT_CLIENT_CONFIG_FILE: '/development/config.json', AXIOM_POOL_CLIENT_CONFIG_FILE: '/development/pool.json',
    AXIOMCLI_TEST_RUNNER: '/fake', AXIOM_BASE_URL: 'https://staging.invalid', PYTHONPATH: '/development/python',
    MACOS_CERTIFICATE_PASSWORD: 'signing-test' };
  const env = installedEnvironment(source, '/acceptance');
  assert.equal(env.HOME, source.HOME);
  assert.equal(env.AXIOM_API_KEY, source.AXIOM_API_KEY);
  assert.equal(env.AXIOM_MODEL, 'direct:qwen3.8-27b');
  assert.match(env.XDG_DATA_HOME, /acceptance.*data/);
  for (const name of ['AXIOM_DIRECT_CLIENT_CONFIG_FILE', 'AXIOM_POOL_CLIENT_CONFIG_FILE', 'AXIOMCLI_TEST_RUNNER',
    'AXIOM_BASE_URL', 'PYTHONPATH', 'MACOS_CERTIFICATE_PASSWORD']) assert.equal(env[name], undefined, name);
  assert.throws(() => installedEnvironment({}, '/acceptance'), /credential/);
});

const image = { requestId: 'image-1', providerId: 'axiom-direct', imagePixels: '2359296',
  state: 'completed', completeness: 'final', responseVerified: true, settled: true, costMicrousd: '8000' };

test('ordinary chat usage cannot substitute for the installed image request', () => {
  assert.throws(() => verifiedImageUsage([{ ...image, imagePixels: null }]), /exactly one image/);
  assert.equal(verifiedImageUsage([{ imagePixels: null }, image]), image);
  assert.throws(() => verifiedImageUsage([image, { ...image, requestId: 'image-2' }]), /exactly one image/);
});

test('image acceptance requires authenticated completion and settlement independently', () => {
  for (const incomplete of [{ settled: false }, { responseVerified: false }, { state: 'failed' },
    { completeness: 'partial' }, { costMicrousd: null }, { providerId: 'fixture' }]) {
    assert.throws(() => verifiedImageUsage([{ ...image, ...incomplete }]), /authenticated, final, settled/);
  }
});

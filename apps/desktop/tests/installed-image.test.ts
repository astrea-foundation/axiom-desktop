import assert from 'node:assert/strict';
import test from 'node:test';
// @ts-expect-error Release scripts run directly in Node without generated declarations.
import { installedEnvironment, installedQaSource, verifiedImageUsage, verifyImageTools, needsImageView, requiredReleaseDefault, verifyDefaultSelection } from '../../../scripts/verify-installed-image.mjs';
import path from 'node:path';
import os from 'node:os';

test('signed installation acceptance uses isolated profiles and drops provider overrides', () => {
  const source = { HOME: '/home/operator', PATH: '/usr/bin', AXIOM_API_KEY: 'automation-test',
    AXIOM_DIRECT_CLIENT_CONFIG_FILE: '/development/config.json', AXIOM_POOL_CLIENT_CONFIG_FILE: '/development/pool.json',
    AXIOMCLI_TEST_RUNNER: '/fake', AXIOM_BASE_URL: 'https://staging.invalid', PYTHONPATH: '/development/python',
    MACOS_CERTIFICATE_PASSWORD: 'signing-test', AXIOM_MODEL: 'direct:qwen3.8-27b' };
  const env = installedEnvironment(source, '/acceptance');
  assert.equal(env.HOME, source.HOME);
  assert.equal(env.AXIOM_API_KEY, undefined);
  assert.equal(env.AXIOM_MODEL, undefined, 'an image-test override must not hide the normal installed default');
  assert.match(env.XDG_DATA_HOME, /acceptance.*data/);
  for (const name of ['AXIOM_DIRECT_CLIENT_CONFIG_FILE', 'AXIOM_POOL_CLIENT_CONFIG_FILE', 'AXIOMCLI_TEST_RUNNER',
    'AXIOM_BASE_URL', 'PYTHONPATH', 'MACOS_CERTIFICATE_PASSWORD']) assert.equal(env[name], undefined, name);
});

test('fresh installed profiles require separate native sessions and Keychain roots before either product runs', () => {
  const root = path.join(os.tmpdir(), 'axiom-installed-test');
  const desktop = { version: '0.1.23', accountId: '11111111-1111-4111-8111-111111111111',
    clientId: 'd'.repeat(43), refreshToken: 'axr_' + 'd'.repeat(43), sessionId: 'd'.repeat(32) };
  const cli = { ...desktop, clientId: 'c'.repeat(43), refreshToken: 'axr_' + 'c'.repeat(43), sessionId: 'c'.repeat(32) };
  const source = { GITHUB_ACTIONS: 'true', RUNNER_ENVIRONMENT: 'github-hosted', RUNNER_OS: 'macOS', RUNNER_TEMP: root,
    AXIOM_INSTALLED_NATIVE_QA: JSON.stringify(desktop), AXIOM_INSTALLED_CLI_NATIVE_QA: JSON.stringify(cli) };
  const first = installedQaSource(source, path.join(root, 'desktop'), 'desktop-chat');
  const second = installedQaSource(source, path.join(root, 'cli'), 'cli');
  assert.equal(first.AXIOM_INSTALLED_NATIVE_QA, source.AXIOM_INSTALLED_NATIVE_QA);
  assert.equal(second.AXIOM_INSTALLED_NATIVE_QA, source.AXIOM_INSTALLED_CLI_NATIVE_QA);
  assert.notEqual(first.RUNNER_TEMP, second.RUNNER_TEMP, 'each fresh vault needs its own temporary Keychain and seed marker');
  assert.equal(source.RUNNER_TEMP, root);
  assert.throws(() => installedQaSource({ ...source, AXIOM_INSTALLED_CLI_NATIVE_QA: undefined }, path.join(root, 'desktop'), 'desktop-chat'), /native-session QA credential/);
  for (const duplicate of [{ clientId: desktop.clientId }, { sessionId: desktop.sessionId },
    { refreshToken: desktop.refreshToken }, { version: '0.1.22' }]) {
    assert.throws(() => installedQaSource({ ...source, AXIOM_INSTALLED_CLI_NATIVE_QA: JSON.stringify({ ...cli, ...duplicate }) },
      path.join(root, 'cli'), 'cli'), /distinct native authorizations/);
  }
  assert.throws(() => installedQaSource(source, root, 'cli'), /inside runner/);
  assert.throws(() => installedQaSource(source, path.dirname(root), 'cli'), /inside runner/);
  assert.throws(() => installedQaSource(source, path.join(root, 'desktop'), 'other'), /Unknown/);
  for (const environment of [installedEnvironment(first, first.RUNNER_TEMP), installedEnvironment(second, second.RUNNER_TEMP)]) {
    assert.equal(environment.AXIOM_INSTALLED_NATIVE_QA, undefined);
    assert.equal(environment.AXIOM_INSTALLED_CLI_NATIVE_QA, undefined, 'QA credentials must never enter the installed child environment');
  }
});

const preferredModel = { id: 'near-abliterated', providerId: 'near',
  upstreamModel: 'deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated', tags: [{ id: 'uncensored' }] };
const fallbackModel = { id: 'tinfoil-deepseek', providerId: 'tinfoil', upstreamModel: 'deepseek-v4-1-flash', tags: [] };

test('release acceptance fails when NEAR discovery vanishes even if Tinfoil and AstreaAI work', () => {
  const imageModel = { id: 'direct:qwen3.8-27b', providerId: 'axiom-direct', upstreamModel: 'qwen3.8-27b', tags: [] };
  assert.throws(() => requiredReleaseDefault([fallbackModel, imageModel]), /Release catalog must contain/);
  assert.equal(requiredReleaseDefault([fallbackModel, preferredModel, imageModel]), preferredModel);
  for (const invalid of [{ ...preferredModel, providerId: 'tinfoil' }, { ...preferredModel, tags: [] },
    { ...preferredModel, upstreamModel: 'deepseek-ai/DeepSeek-V4.1-Flash-1017' }]) {
    assert.throws(() => requiredReleaseDefault([invalid, fallbackModel, imageModel]), /Release catalog must contain/);
  }
});

test('offering the abliterated model is insufficient if a fresh chat defaults to Tinfoil', () => {
  verifyDefaultSelection(preferredModel, [{ id: 'model', currentValue: preferredModel.id }]);
  assert.throws(() => verifyDefaultSelection(preferredModel, [{ id: 'model', currentValue: fallbackModel.id }]), /Fresh installed session/);
  assert.throws(() => verifyDefaultSelection(preferredModel, []), /Fresh installed session/);
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

test('a proposed or failed image view cannot satisfy signed-installation acceptance', () => {
  const generation = { title: 'Creating image: generate', status: 'completed' };
  const view = { title: 'Viewing image: view_image', status: 'completed' };
  verifyImageTools([generation, view]);
  for (const status of ['pending', 'in_progress', 'failed', undefined]) {
    assert.throws(() => verifyImageTools([generation, { ...view, status }]), /successfully complete/);
    assert.throws(() => verifyImageTools([{ ...generation, status }, view]), /successfully complete/);
  }
  assert.throws(() => verifyImageTools([generation]), /successfully complete/);
  assert.throws(() => verifyImageTools([generation, generation, view]), /successfully complete/);
  assert.equal(needsImageView([generation]), true);
  assert.equal(needsImageView([generation, generation]), false);
  assert.equal(needsImageView([generation, view]), false);
  for (const status of ['pending', 'in_progress', 'failed']) {
    assert.equal(needsImageView([{ ...generation, status }]), false);
    assert.equal(needsImageView([generation, { ...view, status }]), false);
  }
  assert.throws(() => verifyImageTools([{ ...generation, title: 'Creating image: regenerate' }, view]), /successfully complete/);
});

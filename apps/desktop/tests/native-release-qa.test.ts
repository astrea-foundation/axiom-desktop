import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
// @ts-expect-error Release scripts run directly in Node.
import { nativeQaFixture, prepareNativeReleaseQa } from '../../../scripts/native-release-qa.mjs';

const fixture = { version: '0.1.21', accountId: '11111111-1111-4111-8111-111111111111',
  clientId: 'c'.repeat(43), refreshToken: 'axr_' + 'r'.repeat(43), sessionId: 'e'.repeat(32) };

test('native acceptance rejects automation keys and non-disposable hosts', () => {
  assert.throws(() => nativeQaFixture({ AXIOM_API_KEY: 'axm_fake' }), /disposable/);
  assert.throws(() => nativeQaFixture({ GITHUB_ACTIONS: 'true', RUNNER_ENVIRONMENT: 'github-hosted', RUNNER_OS: 'macOS', RUNNER_TEMP: '/tmp', AXIOM_API_KEY: 'axm_fake' }), /native-session QA credential/);
});

test('both installed products reuse the rotated native credential without reseeding its old token', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'axiom-native-release-'));
  try {
    const source = { GITHUB_ACTIONS: 'true', RUNNER_ENVIRONMENT: 'github-hosted', RUNNER_OS: 'macOS', RUNNER_TEMP: root, AXIOM_INSTALLED_NATIVE_QA: JSON.stringify(fixture) };
    const calls: { args: string[]; input?: string }[] = [];
    const run = (args: string[], input?: string) => { calls.push({ args, input }); };
    assert.throws(() => prepareNativeReleaseQa(source, { XDG_DATA_HOME: root }, '0.1.22', run), /different release/);
    assert.throws(() => nativeQaFixture({ ...source, RUNNER_ENVIRONMENT: 'self-hosted' }), /disposable/);
    const desktop = { XDG_DATA_HOME: path.join(root, 'desktop/data') };
    const cli = { XDG_DATA_HOME: path.join(root, 'cli/data') };
    assert.deepEqual(prepareNativeReleaseQa(source, desktop, fixture.version, run), { accountId: fixture.accountId, sessionId: fixture.sessionId });
    const seeded = calls.length;
    prepareNativeReleaseQa(source, cli, fixture.version, run);
    assert.equal(calls.length, seeded, 'the consumed refresh token must never be restored over its replacement');
    assert.equal(calls.filter(call => call.input?.includes('add-generic-password')).length, 1);
    assert(calls.some(call => call.input?.startsWith('default-keychain -d user -s ')),
      'native credential lookup selects the default User-domain keychain, independently of its search list');
    assert(calls.every(call => !call.args.join(' ').includes(fixture.refreshToken)), 'secrets must not enter argv');
    for (const environment of [desktop, cli]) assert.equal(readFileSync(path.join(environment.XDG_DATA_HOME, 'axiom/shared/identity/client-id'), 'utf8').trim(), fixture.clientId);
    assert(!readFileSync(path.join(root, 'axiom-installed-native-qa.json'), 'utf8').includes(fixture.refreshToken));
    assert.throws(() => prepareNativeReleaseQa({ ...source, AXIOM_INSTALLED_NATIVE_QA: JSON.stringify({ ...fixture, sessionId: 'f'.repeat(32) }) }, cli, fixture.version, run), /different native QA session/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('native credential setup failures cannot leak a refresh token in release logs', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'axiom-native-release-'));
  try {
    const source = { GITHUB_ACTIONS: 'true', RUNNER_ENVIRONMENT: 'github-hosted', RUNNER_OS: 'macOS', RUNNER_TEMP: root, AXIOM_INSTALLED_NATIVE_QA: JSON.stringify(fixture) };
    assert.throws(() => prepareNativeReleaseQa(source, { XDG_DATA_HOME: path.join(root, 'data') }, fixture.version, () => { throw new Error(fixture.refreshToken); }), error => error instanceof Error && !error.message.includes(fixture.refreshToken));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

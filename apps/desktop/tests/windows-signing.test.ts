import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const require = createRequire(import.meta.url);
const { signingSettings } = require('../scripts/windows-signing.cjs');
const unsignedEnvironment = require('../scripts/unsigned-environment.cjs');
const desktop = fileURLToPath(new URL('..', import.meta.url));
const root = fileURLToPath(new URL('../../..', import.meta.url));
const settings = {
  AXIOM_SIGNING_ENDPOINT: 'https://eus.codesigning.azure.net/',
  AXIOM_SIGNING_ACCOUNT: 'astrealabssigning',
  AXIOM_SIGNING_PROFILE: 'axiom-production',
  AXIOM_SIGNING_PUBLISHER: 'Astrea Labs, Inc.',
};

test('production signer requires complete, regional Azure configuration', () => {
  assert.equal(signingSettings(settings).PUBLISHER, 'Astrea Labs, Inc.');
  for (const key of Object.keys(settings)) {
    assert.throws(() => signingSettings({ ...settings, [key]: '' }), new RegExp(key));
    assert.throws(() => signingSettings({ ...settings, [key]: 'first\nsecond' }));
  }
  for (const endpoint of ['http://eus.codesigning.azure.net', 'https://evil.test', 'https://eus.codesigning.azure.net.evil.test', 'https://eus.codesigning.azure.net/path', 'https://user@eus.codesigning.azure.net']) {
    assert.throws(() => signingSettings({ ...settings, AXIOM_SIGNING_ENDPOINT: endpoint }), /regional/);
  }
  assert.throws(() => signingSettings({ ...settings, AXIOM_SIGNING_PROFILE: '../other-profile' }));
});

test('unsigned packaging strips every signing credential family without mutating its caller', () => {
  const source = { ...settings, PATH: '/bin', CSC_LINK: 'pfx', WIN_CSC_LINK: 'pfx', WIN_CSC_KEY_PASSWORD: 'password', AZURE_CLIENT_SECRET: 'secret', APPLE_ID: 'apple', CSC_IDENTITY_AUTO_DISCOVERY: 'true' };
  assert.deepEqual(unsignedEnvironment(source), { PATH: '/bin', CSC_IDENTITY_AUTO_DISCOVERY: 'false' });
  assert.equal(source.WIN_CSC_LINK, 'pfx');
});

test('electron-builder config loads its real SHA256 signing hook and fails without configuration', () => {
  const code = `const c=require('./electron-builder.windows-signing.cjs'); const hook=require(c.win.signtoolOptions.sign); console.log(JSON.stringify({force:c.forceCodeSigning,publisher:c.win.signtoolOptions.publisherName,hashes:c.win.signtoolOptions.signingHashAlgorithms,hook:typeof hook.sign}));`;
  const result = spawnSync(process.execPath, ['-e', code], { cwd: desktop, env: { ...process.env, ...settings }, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), { force: true, publisher: 'Astrea Labs, Inc.', hashes: ['sha256'], hook: 'function' });
  const missing = spawnSync(process.execPath, ['-e', code], { cwd: desktop, env: unsignedEnvironment(process.env), encoding: 'utf8' });
  assert.notEqual(missing.status, 0);
});

test('unpacked build input is restricted to the explicit unsigned Windows path', () => {
  for (const args of [['--win', '--arm64', '--dir'], ['--mac', '--arm64', '--unsigned', '--dir']]) {
    const result = spawnSync(process.execPath, ['scripts/package-desktop.mjs', ...args], { cwd: desktop, encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /--dir is only for unsigned native Windows/);
  }
});

test('signed release and manual Windows lanes share the protected native-build/sign workflow', () => {
  const read = (file: string) => readFileSync(`${root}/.github/workflows/${file}`, 'utf8');
  for (const file of ['release.yml', 'desktop-packages.yml']) {
    assert.match(read(file), /uses: \.\/\.github\/workflows\/windows-signed\.yml/);
    assert.doesNotMatch(read(file), /WINDOWS_CERTIFICATE/);
  }
  const workflow = read('windows-signed.yml');
  assert.match(workflow, /environment: windows-signing/);
  assert.equal(workflow.match(/id-token: write/g)?.length, 1);
  assert.match(workflow, /git merge-base --is-ancestor HEAD origin\/main/);
  assert.match(workflow, /windows-11-arm/);
  assert.match(workflow, /runs-on: windows-2025/);
  assert.doesNotMatch(workflow, /test-windows-install|windows-signed-candidate/);
  assert.ok(workflow.indexOf('package-signed-windows.ps1') < workflow.indexOf('download-manifest.mjs'));
  assert.match(workflow, /--unsigned --dir/);
});

const pwsh = process.env.AXIOM_TEST_PWSH || 'pwsh';
const hasPowerShell = spawnSync(pwsh, ['-NoProfile', '-Command', '$PSVersionTable.PSVersion.ToString()']).status === 0;
test('PowerShell signing scripts parse and signature checks fail closed', { skip: !hasPowerShell }, () => {
  const result = spawnSync(pwsh, ['-NoProfile', '-NonInteractive', '-File', 'tests/windows-signature-checks.ps1'], { cwd: desktop, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
});

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, chmodSync, symlinkSync, readlinkSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const desktop = fileURLToPath(new URL('..', import.meta.url));
const posix = process.platform !== 'win32';

test('AppImage CLI bypasses Electron and preserves arguments and exit status', { skip: !posix }, () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'axiom package '));
  try {
    mkdirSync(path.join(root, 'resources/bin'), { recursive: true });
    writeFileSync(path.join(root, 'AppRun'), readFileSync(path.join(desktop, 'build/AppRun')), { mode: 0o755 });
    const cli = path.join(root, 'resources/bin/axiomcli');
    writeFileSync(cli, '#!/bin/sh\nprintf "%s\\n" "$@"\nexit 17\n', { mode: 0o755 });
    const result = spawnSync(path.join(root, 'AppRun'), ['--axiom-cli', 'exec', 'spaces $HOME `literal`'], { encoding: 'utf8' });
    assert.equal(result.status, 17, result.stderr);
    assert.equal(result.stdout, 'exec\nspaces $HOME `literal`\n');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('macOS package preserves unrelated commands and accepts its own link on upgrades', { skip: !posix }, () => {
  const volume = mkdtempSync(path.join(os.tmpdir(), 'axiom pkg '));
  try {
    const bin = path.join(volume, 'usr/local/bin');
    mkdirSync(bin, { recursive: true });
    const link = path.join(bin, 'axiomcli');
    const preinstall = path.join(desktop, 'build/pkg-scripts/preinstall');
    const invoke = () => spawnSync('/bin/sh', [preinstall, 'package', '/Applications', volume], { encoding: 'utf8' });
    assert.equal(invoke().status, 0);
    writeFileSync(link, 'another installation');
    assert.notEqual(invoke().status, 0);
    assert.equal(readFileSync(link, 'utf8'), 'another installation');
    rmSync(link);
    symlinkSync('/different/axiomcli', link);
    assert.notEqual(invoke().status, 0);
    rmSync(link);
    symlinkSync('/Applications/Axiom.app/Contents/Resources/bin/axiomcli', link);
    assert.equal(invoke().status, 0);
    assert.equal(readlinkSync(link), '/Applications/Axiom.app/Contents/Resources/bin/axiomcli');
  } finally { rmSync(volume, { recursive: true, force: true }); }
});

test('AppImage install updates in place, registers CLI, and keeps account data', { skip: !posix || process.getuid?.() === 0 }, () => {
  const home = mkdtempSync(path.join(os.tmpdir(), 'axiom home '));
  try {
    const source = path.join(home, 'download.AppImage');
    const data = path.join(home, '.local/share/axiom/accounts/test/desktop');
    mkdirSync(data, { recursive: true });
    writeFileSync(path.join(data, 'state.sqlite3'), 'preserved');
    writeFileSync(source, '#!/bin/sh\nprintf "v1 %s\\n" "$*"\n', { mode: 0o755 });
    const env = { ...process.env, HOME: home, APPIMAGE: source, APPDIR: '' };
    const run = () => spawnSync('/bin/sh', [path.join(desktop, 'scripts/install-appimage.sh')], { env, encoding: 'utf8' });
    assert.equal(run().status, 0);
    const cli = path.join(home, '.local/bin/axiomcli');
    assert.equal(spawnSync(cli, ['--version'], { env, encoding: 'utf8' }).stdout, 'v1 --axiom-cli --version\n');
    writeFileSync(source, '#!/bin/sh\nprintf "v2 %s\\n" "$*"\n');
    assert.equal(run().status, 0);
    assert.equal(spawnSync(cli, ['--version'], { env, encoding: 'utf8' }).stdout, 'v2 --axiom-cli --version\n');
    assert.equal(readFileSync(path.join(data, 'state.sqlite3'), 'utf8'), 'preserved');
    assert.equal(readFileSync(path.join(home, '.bashrc'), 'utf8').split('# Axiom CLI PATH').length, 2);
    rmSync(cli);
    writeFileSync(cli, 'external-cli');
    assert.notEqual(run().status, 0);
    assert.equal(readFileSync(cli, 'utf8'), 'external-cli');
  } finally { rmSync(home, { recursive: true, force: true }); }
});

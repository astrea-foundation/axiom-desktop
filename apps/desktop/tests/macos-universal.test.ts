import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import test from 'node:test';

const require = createRequire(import.meta.url);
const builder = createRequire(createRequire(require.resolve('electron-builder')).resolve('app-builder-lib'));
const universal = createRequire(builder.resolve('@electron/universal'));
const module = universal('minimatch');
const minimatch = typeof module === 'function' ? module : module.minimatch;
const config = builder('js-yaml').load(readFileSync(fileURLToPath(new URL('../electron-builder.yml', import.meta.url)), 'utf8'));
const matches = (file: string) => minimatch(file, config.mac.x64ArchFiles, {matchBase: true});

test('universal packaging preserves pre-signed runtime libraries in hidden vendor directories', () => {
  for (const file of [
    'Contents/Resources/chutes/runtime/arm64/_internal/PIL/.dylibs/libXau.6.dylib',
    'Contents/Resources/chutes/runtime/x64/_internal/PIL/.dylibs/libjpeg.8.dylib',
    'Contents/Resources/direct/darwin-arm64/runtime/_internal/PIL/.dylibs/libXau.6.dylib',
    'Contents/Resources/direct/darwin-x64/runtime/_internal/libcrypto.3.dylib',
    'Contents/Resources/direct/darwin-arm64/runtime/axiom-direct-verify',
  ]) assert.equal(matches(file), true, file);
});

test('runtime preservation does not exempt the app or Rust sidecars from universal merging', () => {
  for (const file of [
    'Contents/MacOS/Axiom',
    'Contents/Resources/bin/axiomcli',
    'Contents/Resources/bin/axiom-proxy',
    'Contents/Resources/app.asar.unpacked/native.dylib',
    'Contents/Frameworks/Electron Framework.framework/Electron Framework',
  ]) assert.equal(matches(file), false, file);
});

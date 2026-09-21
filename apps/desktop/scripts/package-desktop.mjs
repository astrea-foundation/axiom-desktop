import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import unsignedEnvironment from './unsigned-environment.cjs';

const root = fileURLToPath(new URL('../../..', import.meta.url));
const desktop = path.join(root, 'apps/desktop');
const args = process.argv.slice(2);
const platform = args.find(value => ['--linux', '--mac', '--win'].includes(value));
const architecture = args.find(value => ['--x64', '--arm64'].includes(value));
const unsigned = args.includes('--unsigned');
const unpacked = args.includes('--dir');
if (!platform || !architecture || args.some(arg => !['--linux','--mac','--win','--x64','--arm64','--unsigned','--dir'].includes(arg))) {
  throw new Error('Usage: package-desktop.mjs --linux|--mac|--win --x64|--arm64 [--unsigned] [--dir]');
}
if (unpacked && (platform !== '--win' || !unsigned)) throw new Error('--dir is only for unsigned native Windows build inputs; it is not a release artifact.');
const cliVersion = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(path.join(root,'apps/axiomcli/Cargo.toml'),'utf8'))?.[1];
const hostPlatform = { linux: '--linux', darwin: '--mac', win32: '--win' }[process.platform];
if (platform !== hostPlatform) throw new Error('Build desktop installers on a native runner for that operating system.');
const sidecar = path.join(root, 'target/release', platform === '--win' ? 'axiomcli.exe' : 'axiomcli');
const binary = readFileSync(sidecar);
const arm = architecture === '--arm64';
if (platform === '--linux') {
  if (binary.subarray(0,4).toString('hex') !== '7f454c46' || binary.readUInt16LE(18) !== (arm ? 183 : 62)) throw new Error('AxiomCLI ELF architecture does not match the package.');
} else if (platform === '--mac') {
  if (binary.readUInt32LE(0) !== 0xfeedfacf || binary.readUInt32LE(4) !== (arm ? 0x0100000c : 0x01000007)) throw new Error('AxiomCLI Mach-O architecture does not match the package.');
} else {
  const pe = binary.readUInt32LE(0x3c);
  if (binary.readUInt16LE(0) !== 0x5a4d || binary.readUInt32LE(pe) !== 0x4550 || binary.readUInt16LE(pe+4) !== (arm ? 0xaa64 : 0x8664)) throw new Error('AxiomCLI PE architecture does not match the package.');
}
const reported = spawnSync(sidecar, ['--version'], { encoding: 'utf8' });
if (reported.status !== 0 || reported.stdout.trim() !== `axiomcli ${cliVersion}`) throw new Error('Build the matching release AxiomCLI before packaging Desktop.');
function run(command, values, env = process.env) {
  const result = spawnSync(command, values, { cwd: desktop, stdio: 'inherit', env, shell: process.platform === 'win32' && command === 'pnpm' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
if (platform === '--win' && !unsigned) {
  if (process.arch !== 'x64') throw new Error('Use the native-build/x64-sign workflow for ARM64 production packages.');
  run(process.execPath, ['scripts/package-desktop.mjs', '--win', architecture, '--unsigned', '--dir']);
  run('pwsh', ['-NoProfile', '-NonInteractive', '-File', 'scripts/package-signed-windows.ps1', '-Arch', architecture.slice(2)]);
  process.exit(0);
}
run(process.execPath, ['scripts/set-release-version.mjs', cliVersion]);
run('pnpm', ['build']);
const builder = [platform, architecture, '--publish', 'never'];
const env = unsigned ? unsignedEnvironment(process.env) : { ...process.env };
if (unsigned) {
  builder.push('--config.forceCodeSigning=false');
  // The unsigned option is explicit; production defaults still require signing.
  if (platform === '--win') builder.push('--config.win.signExecutable=false');
  if (platform === '--mac') builder.push('--config.mac.identity=-', '--config.mac.notarize=false', '--config.dmg.sign=false', '--config.mac.entitlements=build/entitlements.unsigned.plist', '--config.mac.entitlementsInherit=build/entitlements.unsigned.plist');
}
if (unpacked) {
  run('pnpm', ['exec', 'electron-builder', ...builder, '--dir', '--config.extraMetadata.axiomUpdateFormat=exe'], env);
  process.exit(0);
}
// Each installer carries its own format inside app.asar/package.json. A single
// multi-target build cannot distinguish a DMG app from the same app in a ZIP.
const targets = platform === '--mac' ? ['dmg', 'zip', 'pkg'] : platform === '--win' ? ['nsis'] : ['AppImage', 'deb', 'pacman'];
for (const target of targets) {
  const format = target === 'nsis' ? 'exe' : target;
  run('pnpm', ['exec', 'electron-builder', builder[0], target, ...builder.slice(1), `--config.extraMetadata.axiomUpdateFormat=${format}`], env);
}

import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { readFileSync, copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import unsignedEnvironment from './unsigned-environment.cjs';
import {verifyNativeInput} from '../../../scripts/native-input.mjs';

const root = fileURLToPath(new URL('../../..', import.meta.url));
const desktop = path.join(root, 'apps/desktop');
const args = process.argv.slice(2);
const platform = args.find(value => ['--linux', '--mac', '--win'].includes(value));
const architecture = args.find(value => ['--x64', '--arm64', '--universal'].includes(value));
const combined = architecture === '--universal';
const unsigned = args.includes('--unsigned');
const unpacked = args.includes('--dir');
if (!platform || !architecture || args.some(arg => !['--linux','--mac','--win','--x64','--arm64','--universal','--unsigned','--dir'].includes(arg))) {
  throw new Error('Usage: package-desktop.mjs --linux|--mac|--win --x64|--arm64|--universal [--unsigned] [--dir]');
}
if (combined && platform === '--linux') throw new Error('Linux packages use their native architecture');
if (unpacked && (platform !== '--win' || !unsigned)) throw new Error('--dir is only for unsigned native Windows build inputs; it is not a release artifact.');
const cliVersion = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(path.join(root,'apps/axiomcli/Cargo.toml'),'utf8'))?.[1];
const hostPlatform = { linux: '--linux', darwin: '--mac', win32: '--win' }[process.platform];
if (platform !== hostPlatform) throw new Error('Build desktop installers on a native runner for that operating system.');
const {default: assertArchitecture} = await import('../../../scripts/native-binary.cjs');
const nativeInput = path.join(root, 'target/release');
const name = platform === '--win' ? 'axiomcli.exe' : 'axiomcli';
if (combined) {
  for (const arch of ['x64','arm64']) await verifyNativeInput(path.join(nativeInput,arch), platform.slice(2), arch);
} else if (platform !== '--linux') {
  mkdirSync(path.join(nativeInput,architecture.slice(2)), {recursive:true});
  copyFileSync(path.join(nativeInput,name), path.join(nativeInput,architecture.slice(2),name));
}
const sidecar = path.join(nativeInput, ...(combined ? [process.arch] : []), name);
assertArchitecture(sidecar, platform.slice(2), combined ? process.arch : architecture.slice(2));
const reported = spawnSync(sidecar, ['--version'], { encoding: 'utf8' });
if (reported.status !== 0 || reported.stdout.trim() !== `axiomcli ${cliVersion}`) throw new Error('Build the matching release AxiomCLI before packaging Desktop.');
if (!unsigned) {
  const keys = spawnSync(sidecar, ['update', '--trust-keys'], {encoding: 'utf8'});
  if (keys.status !== 0 || !process.env.AXIOM_UPDATE_PUBLIC_KEYS || keys.stdout.trim() !== process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('Compile the release with its trusted AXIOM_UPDATE_PUBLIC_KEYS before packaging');
}
function run(command, values, env = process.env) {
  const result = spawnSync(command, values, { cwd: desktop, stdio: 'inherit', env, shell: process.platform === 'win32' && command === 'pnpm' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
if (platform === '--win' && !unsigned) {
  if (process.arch !== 'x64') throw new Error('Windows signing requires an x64 signing host.');
}
run(process.execPath, ['scripts/set-release-version.mjs', cliVersion]);
run('pnpm', ['build']);
const builder = [platform, ...(combined && platform === '--win' ? ['--x64','--arm64'] : [architecture]), '--publish', 'never', `--config.extraMetadata.axiomUpdateChannel=${process.env.AXIOM_UPDATE_PUBLIC_KEYS ? 'stable' : 'preview'}`];
if (combined && platform === '--win') builder.push('--config.artifactName=Axiom-${version}-win-universal.${ext}');
if (platform === '--win' && !unsigned) builder.push('--config', 'electron-builder.windows-signing.cjs');
// Notarize the outer PKG below, including its signed app payload. Submitting the
// app separately first adds a redundant Apple queue wait for each architecture.
if (platform === '--mac') builder.push('--config.mac.notarize=false');
const env = unsigned ? unsignedEnvironment(process.env) : { ...process.env };
if (combined && platform === '--win') env.AXIOM_NATIVE_INPUT = nativeInput;
if (platform === '--win') {
  env.ELECTRON_BUILDER_7Z_FILTER = 'BCJ';
  for (const name of ['CSC_LINK','CSC_KEY_PASSWORD','WIN_CSC_LINK','WIN_CSC_KEY_PASSWORD']) delete env[name];
}
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
// Each installer carries its own format inside app.asar/package.json.
const targets = platform === '--mac' ? ['pkg'] : platform === '--win' ? ['nsis'] : ['AppImage', 'deb', 'pacman'];
for (const target of targets) {
  const format = target === 'nsis' ? 'exe' : target;
  run('pnpm', ['exec', 'electron-builder', builder[0], target, ...builder.slice(1), `--config.extraMetadata.axiomUpdateFormat=${format}`], env);
  if (platform === '--win') {
    for (const arch of combined ? ['x64','arm64'] : [architecture.slice(2)]) {
      run('pwsh', ['-NoProfile','-NonInteractive','-File','scripts/verify-windows-package.ps1', '-Unpacked',`dist/${arch === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked'}`, '-Arch',arch,'-Version',cliVersion,'-Installer',`dist/Axiom-${cliVersion}-win-${architecture.slice(2)}.exe`, ...(combined ? ['-Combined'] : []), ...(!unsigned ? ['-Publisher',env.AXIOM_SIGNING_PUBLISHER] : [])],env);
    }
  }
  if (platform === '--mac' && !unsigned) {
    const installer = path.join(desktop,'dist',`Axiom-${cliVersion}-mac-${architecture.slice(2)}.pkg`);
    run('xcrun',['notarytool','submit',installer,'--apple-id',env.APPLE_ID,'--password',env.APPLE_APP_SPECIFIC_PASSWORD,'--team-id',env.APPLE_TEAM_ID,'--wait'],env);
    run('xcrun',['stapler','staple',installer],env);
  }
}

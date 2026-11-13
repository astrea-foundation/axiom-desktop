import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {chmod, copyFile, mkdir, readFile, readdir, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import assertArchitecture from './native-binary.cjs';
import bundleWindowsRuntime from '../apps/desktop/scripts/bundle-windows-runtime.cjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const version = /^version\s*=\s*"([^"]+)"/m.exec(await readFile(path.join(root, 'apps/axiomcli/Cargo.toml'), 'utf8'))[1];
const revision = () => execFileSync('git', ['rev-parse', 'HEAD'], {cwd:root, encoding:'utf8'}).trim();
async function inventory(directory, prefix = '') {
  const hashes = {};
  for (const entry of await readdir(path.join(directory, prefix), {withFileTypes:true})) {
    const name = prefix + entry.name;
    if (name === 'build.json') continue;
    if (entry.isDirectory()) Object.assign(hashes, await inventory(directory, `${name}/`));
    else if (entry.isFile()) hashes[name] = createHash('sha256').update(await readFile(path.join(directory, name))).digest('hex');
    else throw new Error(`Unsupported native input entry: ${name}`);
  }
  return Object.fromEntries(Object.entries(hashes).sort());
}

export async function verifyNativeInput(directory, platform, arch) {
  const receipt = JSON.parse((await readFile(path.join(directory, 'build.json'), 'utf8')).replace(/^\uFEFF/, ''));
  if (receipt.version !== version || receipt.revision !== revision() || receipt.platform !== platform || receipt.arch !== arch || receipt.keys !== process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('Native build receipt does not match this release');
  if (JSON.stringify(receipt.sha256) !== JSON.stringify(await inventory(directory))) throw new Error('Native build input changed');
  for (const name of ['axiomcli', 'axiom-proxy']) {
    const binary = path.join(directory, `${name}${platform === 'win' ? '.exe' : ''}`);
    assertArchitecture(binary, platform, arch);
    await chmod(binary, 0o755);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [platform, arch, output] = process.argv.slice(2);
  if (!['mac','win'].includes(platform) || !['x64','arm64'].includes(arch) || !output || arch !== process.arch || platform !== ({darwin:'mac',win32:'win'})[process.platform]) throw new Error('Usage: native-input.mjs mac|win x64|arm64 OUTPUT (on the matching native host)');
  if (!process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('Release update keys are required');
  await mkdir(output, {recursive:true});
  const extension = platform === 'win' ? '.exe' : '';
  const cli = path.join(root, 'target/release', `axiomcli${extension}`);
  const run = args => execFileSync(cli, args, {encoding:'utf8'}).trim();
  if (run(['--version']) !== `axiomcli ${version}` || run(['update','--trust-keys']) !== process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('Native binary version or update trust mismatch');
  for (const name of ['axiomcli','axiom-proxy']) {
    const binary = `${name}${extension}`;
    assertArchitecture(path.join(root, 'target/release', binary), platform, arch);
    await copyFile(path.join(root, 'target/release', binary), path.join(output, binary));
  }
  if (platform === 'win') {
    // Capture the native runner's redistributables; the signing host need not
    // have the other architecture's Visual Studio components installed.
    const runtime = path.join(output, 'runtime');
    await mkdir(path.join(runtime, 'bin'), {recursive:true});
    await copyFile(cli, path.join(runtime, 'bin/axiomcli.exe'));
    await bundleWindowsRuntime(runtime);
    const {rm} = await import('node:fs/promises');
    await rm(path.join(runtime, 'bin/axiomcli.exe'));
  }
  await writeFile(path.join(output, 'build.json'), JSON.stringify({version, revision:revision(), platform, arch, keys:process.env.AXIOM_UPDATE_PUBLIC_KEYS, sha256:await inventory(output)}, null, 2)+'\n');
}

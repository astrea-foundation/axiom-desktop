const { cp, lstat, readFile, readdir, readlink, realpath } = require('node:fs/promises');
const { createHash } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const path = require('node:path');

const digest = data => createHash('sha256').update(data).digest('hex');
const contained = (root, file) => file.startsWith(root + path.sep);

async function validateBundle(source, { platform, architecture, channel, signed = false, teamId, publisher = process.env.AXIOM_SIGNING_PUBLISHER } = {}) {
  if (!['darwin', 'linux', 'win32'].includes(platform)) throw new Error('Chutes helper platform is not qualified');
  const root = await realpath(source);
  const inventoryPath = path.join(root, 'bundle.json');
  if (!(await lstat(inventoryPath)).isFile() || (await lstat(inventoryPath)).isSymbolicLink()) throw new Error('Chutes bundle inventory must be a regular file');
  const manifestBytes = await readFile(inventoryPath);
  if (manifestBytes.length > 2 * 1024 * 1024) throw new Error('Chutes bundle inventory is too large');
  const manifest = JSON.parse(manifestBytes);
  const windowsEmulation = platform === 'win32' && architecture === 'arm64' && manifest.architecture === 'x64' && manifest.windows_arm64_emulation === true;
  const universalMac = platform === 'darwin' && manifest.architecture === 'universal' && ['arm64','x64','universal'].includes(architecture);
  if (Object.hasOwn(manifest,'windows_arm64_emulation') && (platform !== 'win32' || manifest.architecture !== 'x64' || manifest.windows_arm64_emulation !== true)) throw new Error('Invalid Windows helper execution target');
  if (manifest.schema !== 'axiom-chutes-helper-bundle-v1' || manifest.platform !== platform ||
      (manifest.architecture !== architecture && !windowsEmulation && !universalMac) || !['staging','production'].includes(channel) || manifest.channel !== channel ||
      !Array.isArray(manifest.files) || manifest.files.length < 5 || manifest.files.length > 4096 ||
      manifest.self_test?.passed !== true || manifest.self_test?.frozen !== true) throw new Error('Chutes bundle target or self test differs');
  const seen = new Set(['bundle.json']);
  let total = 0;
  for (const row of manifest.files) {
    if (typeof row.path !== 'string' || row.path.includes('\\') || row.path.startsWith('/') ||
        row.path.split('/').some(part => !part || part === '.' || part === '..') || seen.has(row.path)) throw new Error('Invalid Chutes inventory path');
    seen.add(row.path);
    const file = path.join(root, row.path);
    const metadata = await lstat(file);
    if (!contained(root, await realpath(file))) throw new Error('Chutes bundle entry escapes its directory');
    if (Object.hasOwn(row, 'symlink')) {
      if (!metadata.isSymbolicLink() || path.isAbsolute(row.symlink) || await readlink(file) !== row.symlink) throw new Error('Chutes runtime symlink differs');
      continue;
    }
    if (!metadata.isFile() || metadata.isSymbolicLink() || !Number.isSafeInteger(row.bytes) || row.bytes < 0 ||
        row.bytes !== metadata.size || !/^[0-9a-f]{64}$/.test(row.sha256)) throw new Error('Chutes bundle file differs');
    total += metadata.size;
    if (total > 256 * 1024 * 1024) throw new Error('Chutes helper exceeds the client bundle limit');
    const bytes = await readFile(file);
    if (digest(bytes) !== row.sha256) throw new Error('Chutes bundle checksum differs');
    const macho = bytes.length >= 4 && ['cffaedfe','cefaedfe','cafebabe','cafebabf'].includes(bytes.subarray(0,4).toString('hex'));
    if (signed && platform === 'darwin' && macho) {
      if (!teamId || !/^[A-Z0-9]{10}$/.test(teamId)) throw new Error('Chutes signed macOS bundle requires APPLE_TEAM_ID');
      const verify = spawnSync('codesign', ['--verify', '--strict', file], {encoding:'utf8'});
      const display = spawnSync('codesign', ['--display', '--verbose=4', file], {encoding:'utf8'});
      if (verify.status !== 0 || display.status !== 0 || !display.stderr.split('\n').includes(`TeamIdentifier=${teamId}`)) throw new Error('Chutes runtime must be signed by the installer team before packaging');
    }
    if (signed && platform === 'win32' && /\.(exe|dll|pyd)$/i.test(file)) {
      if (typeof publisher !== 'string' || !publisher.trim()) throw new Error('Chutes signed Windows bundle requires its expected publisher');
      const result = spawnSync('pwsh',['-NoProfile','-NonInteractive','-File',path.join(__dirname,'verify-chutes-signature.ps1'),'-FilePath',file,'-Publisher',publisher],{encoding:'utf8'});
      if (result.status !== 0) throw new Error('Chutes runtime requires a timestamped signature from the installer publisher before packaging');
    }
  }
  async function inspect(directory) {
    for (const entry of await readdir(directory, {withFileTypes:true})) {
      const file = path.join(directory, entry.name);
      const name = path.relative(root, file).split(path.sep).join('/');
      if (entry.isDirectory()) await inspect(file);
      else if (!seen.has(name)) throw new Error('Unexpected file in Chutes helper bundle');
    }
  }
  await inspect(root);
  const native = JSON.parse(await readFile(path.join(root, 'native.json'), 'utf8'));
  const bridge = JSON.parse(await readFile(path.join(root, 'bridge.json'), 'utf8'));
  const release = JSON.parse(await readFile(path.join(root, 'release.json'), 'utf8'));
  const authority = JSON.parse(await readFile(path.join(root, 'authority.json'), 'utf8'));
  if (Object.keys(native).sort().join(',') !== 'config,helper,helper_sha256,maximum_charge_microusd,release_authority_fingerprint' ||
      native.helper !== `runtime/axiom-chutes-helper${platform === 'win32' ? '.exe' : ''}` || native.config !== 'bridge.json' ||
      native.helper_sha256 !== digest(await readFile(path.join(root, native.helper))) ||
      !Number.isSafeInteger(native.maximum_charge_microusd) || native.maximum_charge_microusd < 0 ||
      Object.keys(bridge).sort().join(',') !== 'chute_id,hosted_relay,release_verification_config' || bridge.hosted_relay !== true ||
      bridge.release_verification_config !== 'release.json' || !/^[0-9a-f-]{36}$/.test(bridge.chute_id) ||
      Object.keys(release).sort().join(',') !== 'authority,user_state' || release.authority !== 'authority.json' || release.user_state !== true ||
      Object.keys(authority).sort().join(',') !== 'channel,maximum_policy_lifetime_seconds,minimum_policy_sequence,public_key,repository,schema' ||
      authority.schema !== 'axiom-inference-publisher-v1' || !/^[0-9a-f]{64}$/.test(authority.public_key) ||
      !Number.isSafeInteger(authority.minimum_policy_sequence) || authority.minimum_policy_sequence < 1 ||
      !Number.isSafeInteger(authority.maximum_policy_lifetime_seconds) || authority.maximum_policy_lifetime_seconds < 300 || authority.maximum_policy_lifetime_seconds > 7*86400 ||
      authority.repository !== manifest.repository || authority.channel !== channel ||
      native.release_authority_fingerprint !== digest(Buffer.from(authority.public_key, 'hex')) ||
      native.release_authority_fingerprint !== manifest.authority_fingerprint) throw new Error('Chutes public trust configuration differs');
  const assertArchitecture = require('../../../scripts/native-binary.cjs');
  assertArchitecture(path.join(root,native.helper), {darwin:'mac',linux:'linux',win32:'win'}[platform], manifest.architecture);
  if (universalMac) {
    if (!manifest.native_children || Object.keys(manifest.native_children).sort().join(',') !== 'arm64,x64') throw new Error('Universal helper requires both pinned native children');
    for (const child of ['arm64','x64']) {
      const executable = path.join(root,'runtime',child,'axiom-chutes-helper');
      if (manifest.native_children[child] !== digest(await readFile(executable))) throw new Error('Universal native child checksum differs');
      assertArchitecture(executable,'mac',child);
    }
  }
  return manifest;
}

async function bundleChutesRuntime(resources, source, options) {
  const root = await realpath(source);
  await validateBundle(root, options);
  const target = path.join(resources, 'chutes');
  try { await lstat(target); throw new Error('Refusing to replace an existing packaged Chutes runtime'); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  await cp(root, target, {recursive:true, dereference:false, verbatimSymlinks:true});
  await validateBundle(target, options);
  return target;
}

async function verifyPackagedBundle(resources, source, options) {
  const target = path.join(resources, 'chutes');
  if (!(await lstat(target)).isDirectory()) throw new Error('Packaged Chutes runtime must be an ordinary directory');
  await validateBundle(source, options);
  await validateBundle(target, options);
  if (!(await readFile(path.join(source,'bundle.json'))).equals(await readFile(path.join(target,'bundle.json')))) {
    throw new Error('Packaged Chutes runtime differs from its validated input');
  }
  return target;
}

module.exports = bundleChutesRuntime;
module.exports.validateBundle = validateBundle;
module.exports.verifyPackagedBundle = verifyPackagedBundle;

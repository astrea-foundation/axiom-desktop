import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { chmod, mkdtemp, mkdir, readFile, rename, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

const require = createRequire(import.meta.url);
const bundleRuntime = require('../scripts/bundle-chutes-runtime.cjs');
const options = {platform:'darwin', architecture:'arm64', channel:'staging'};
const digest = (data: Buffer) => createHash('sha256').update(data).digest('hex');

async function fixture(root: string) {
  const source = path.join(root, 'source');
  await mkdir(path.join(source, 'runtime'), {recursive:true});
  const binary = Buffer.alloc(64);
  binary.writeUInt32LE(0xfeedfacf,0);
  binary.writeUInt32LE(0x0100000c,4);
  await writeFile(path.join(source,'runtime/axiom-chutes-helper'),binary);
  await chmod(path.join(source,'runtime/axiom-chutes-helper'),0o755);
  const fingerprint = digest(Buffer.from('ab'.repeat(32),'hex'));
  const values = {
    'authority.json': {schema:'axiom-inference-publisher-v1',repository:'example/inference',channel:'staging',public_key:'ab'.repeat(32),minimum_policy_sequence:1,maximum_policy_lifetime_seconds:86400},
    'release.json': {authority:'authority.json',user_state:true},
    'bridge.json': {hosted_relay:true,chute_id:'12345678-1234-1234-1234-123456789012',release_verification_config:'release.json'},
    'native.json': {helper:'runtime/axiom-chutes-helper',helper_sha256:digest(binary),config:'bridge.json',release_authority_fingerprint:fingerprint,maximum_charge_microusd:100000},
  };
  const files: Array<{path:string; bytes:number; sha256:string}> = [{path:'runtime/axiom-chutes-helper',bytes:binary.length,sha256:digest(binary)}];
  for (const [name, value] of Object.entries(values)) {
    const bytes = Buffer.from(JSON.stringify(value));
    await writeFile(path.join(source,name),bytes);
    files.push({path:name,bytes:bytes.length,sha256:digest(bytes)});
  }
  const manifest = {schema:'axiom-chutes-helper-bundle-v1',platform:'darwin',architecture:'arm64',repository:'example/inference',channel:'staging',authority_fingerprint:fingerprint,self_test:{passed:true,frozen:true},files};
  await writeFile(path.join(source,'bundle.json'),JSON.stringify(manifest));
  return {source,manifest};
}

test('Windows Arm requires the explicit x64-emulation payload and correct executable target', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(),'axiom-chutes-windows-'));
  try {
    const {source,manifest} = await fixture(root);
    await rename(path.join(source,'runtime/axiom-chutes-helper'),path.join(source,'runtime/axiom-chutes-helper.exe'));
    const binary = Buffer.alloc(128);
    binary.writeUInt16LE(0x5a4d,0); binary.writeUInt32LE(64,0x3c);
    binary.writeUInt32LE(0x4550,64); binary.writeUInt16LE(0x8664,68);
    await writeFile(path.join(source,'runtime/axiom-chutes-helper.exe'),binary);
    const nativePath = path.join(source,'native.json');
    const native = JSON.parse(await readFile(nativePath,'utf8'));
    native.helper = 'runtime/axiom-chutes-helper.exe'; native.helper_sha256 = digest(binary);
    const bytes = Buffer.from(JSON.stringify(native)); await writeFile(nativePath,bytes);
    Object.assign(manifest,{platform:'win32',architecture:'x64',windows_arm64_emulation:true});
    Object.assign(manifest.files.find(row=>row.path==='native.json')!,{bytes:bytes.length,sha256:digest(bytes)});
    Object.assign(manifest.files.find(row=>row.path.startsWith('runtime/'))!,{path:native.helper,bytes:binary.length,sha256:digest(binary)});
    const save = () => writeFile(path.join(source,'bundle.json'),JSON.stringify(manifest));
    await save();
    await bundleRuntime.validateBundle(source,{platform:'win32',architecture:'arm64',channel:'staging'});
    await bundleRuntime.validateBundle(source,{platform:'win32',architecture:'x64',channel:'staging'});
    Object.assign(manifest,{windows_arm64_emulation:false}); await save();
    await assert.rejects(bundleRuntime.validateBundle(source,{platform:'win32',architecture:'arm64',channel:'staging'}));
    Object.assign(manifest,{windows_arm64_emulation:true}); await save();
    await assert.rejects(bundleRuntime.validateBundle(source,{platform:'win32',architecture:'x64',channel:'staging',signed:true}));
  } finally { await rm(root,{recursive:true,force:true}); }
});

test('copies a complete helper payload without development checkout paths', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(),'axiom-chutes-bundle-'));
  try {
    const {source} = await fixture(root);
    const resources = path.join(root,'app resources');
    const target = await bundleRuntime(resources,source,options);
    const loaded = JSON.parse(await readFile(path.join(target,'native.json'),'utf8'));
    assert.equal(loaded.helper,'runtime/axiom-chutes-helper');
    assert.equal(loaded.config,'bridge.json');
    assert.equal(loaded.python,undefined);
    await assert.rejects(bundleRuntime(resources,source,options),/existing packaged/);
  } finally { await rm(root,{recursive:true,force:true}); }
});

test('the final packaging pass validates the existing helper without copying over it', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(),'axiom-chutes-final-'));
  try {
    const {source} = await fixture(root);
    const resources = path.join(root,'resources');
    const target = await bundleRuntime(resources,source,options);
    const before = await readFile(path.join(target,'bundle.json'));
    assert.equal(await bundleRuntime.verifyPackagedBundle(resources,source,options),target);
    assert.deepEqual(await readFile(path.join(target,'bundle.json')),before);
    await assert.rejects(bundleRuntime(resources,source,options),/existing packaged/);
    await writeFile(path.join(target,'unexpected'),'must not be removed or accepted');
    await assert.rejects(bundleRuntime.verifyPackagedBundle(resources,source,options),/Unexpected file/);
    assert.equal(await readFile(path.join(target,'unexpected'),'utf8'),'must not be removed or accepted');
  } finally { await rm(root,{recursive:true,force:true}); }
});

test('final validation rejects another self-consistent helper and still requires signatures', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(),'axiom-chutes-final-binding-'));
  try {
    const {source,manifest} = await fixture(root);
    const resources = path.join(root,'resources');
    const target = await bundleRuntime(resources,source,options);
    await assert.rejects(bundleRuntime.verifyPackagedBundle(resources,source,{...options,signed:true,teamId:'ABCDEFGHIJ'}));
    const nativePath = path.join(target,'native.json');
    const native = JSON.parse(await readFile(nativePath,'utf8'));
    native.maximum_charge_microusd = 1;
    const bytes = Buffer.from(JSON.stringify(native));await writeFile(nativePath,bytes);
    Object.assign(manifest.files.find(row=>row.path==='native.json')!,{bytes:bytes.length,sha256:digest(bytes)});
    await writeFile(path.join(target,'bundle.json'),JSON.stringify(manifest));
    await bundleRuntime.validateBundle(target,options);
    await assert.rejects(bundleRuntime.verifyPackagedBundle(resources,source,options),/differs from its validated input/);
  } finally { await rm(root,{recursive:true,force:true}); }
});

for (const attack of ['code','extra','traversal','duplicate','symlink','inventory-symlink','architecture','channel','unsigned-runtime']) {
  test(`rejects ${attack} before installing the helper`, {
    skip: process.platform === 'win32' && ['symlink','inventory-symlink'].includes(attack),
  }, async () => {
    const root = await mkdtemp(path.join(os.tmpdir(),'axiom-chutes-bundle-'));
    try {
      const {source,manifest} = await fixture(root);
      const selected = {...options};
      if (attack === 'code') await writeFile(path.join(source,'runtime/axiom-chutes-helper'),'changed helper');
      if (attack === 'extra') await writeFile(path.join(source,'private-provider-key'),'must never ship');
      if (attack === 'traversal') manifest.files[0].path = '../outside';
      if (attack === 'duplicate') manifest.files.push(manifest.files[0]);
      if (attack === 'symlink') {
        await writeFile(path.join(root,'outside'),'outside');
        await symlink(path.join(root,'outside'),path.join(source,'escape'));
        (manifest.files as unknown[]).push({path:'escape',symlink:path.join(root,'outside')});
      }
      if (attack === 'architecture') selected.architecture = 'x64';
      if (attack === 'channel') selected.channel = 'production';
      await writeFile(path.join(source,'bundle.json'),JSON.stringify(manifest));
      if (attack === 'inventory-symlink') {
        await writeFile(path.join(root,'outside-inventory'),JSON.stringify(manifest));
        await rm(path.join(source,'bundle.json'));
        await symlink(path.join(root,'outside-inventory'),path.join(source,'bundle.json'));
      }
      await assert.rejects(bundleRuntime(path.join(root,'resources'),source,
        attack === 'unsigned-runtime' ? {...selected,signed:true,teamId:'ABCDEFGHIJ'} : selected));
    } finally { await rm(root,{recursive:true,force:true}); }
  });
}

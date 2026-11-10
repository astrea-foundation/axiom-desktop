import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import bundle from './direct-bundle.cjs';

// Format-only packaging fixtures exercise inventory checks, never TEE acceptance.
async function fixture(t,target='linux-x64') {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-bundle-'));
  t.after(()=>fs.rm(root,{recursive:true,force:true}));
  await fs.mkdir(path.join(root,'runtime'));
  const binary=Buffer.alloc(64);
  if(target.startsWith('darwin-')) {binary.writeUInt32LE(0xfeedfacf);binary.writeUInt32LE(target==='darwin-arm64'?0x0100000c:0x01000007,4);}
  else {binary.set(Buffer.from('7f454c460201','hex'));binary.writeUInt16LE(62,18);}
  await fs.writeFile(path.join(root,'runtime/axiom-direct-verify'),binary);
  const config={account:{origin:'https://api.axiom.stream'},verifier:{binary:'runtime/axiom-direct-verify',binary_sha256:bundle.digest(binary)}};
  await fs.writeFile(path.join(root,'native.json'),JSON.stringify(config));
  async function stamp() {await fs.writeFile(path.join(root,'bundle.json'),JSON.stringify({schemaVersion:1,provider:'axiom-direct',target,nativeClean:true,files:await bundle.inventory(root)}));}
  await stamp();return {root,config,stamp};
}
test('a changed runtime fails the committed inventory',async t=>{
  const {root}=await fixture(t);await bundle.inspectBundle(root,'linux-x64');
  await fs.appendFile(path.join(root,'runtime/axiom-direct-verify'),'changed');
  await assert.rejects(bundle.inspectBundle(root,'linux-x64'),/inventory mismatch/);
});
test('installed bundles reject operator keys and escaped verifier paths',async t=>{
  const {root,config,stamp}=await fixture(t);
  config.issuer_key_file='/private/operator.pem';await fs.writeFile(path.join(root,'native.json'),JSON.stringify(config));await stamp();
  await assert.rejects(bundle.inspectBundle(root,'linux-x64'),/account configuration/);
  delete config.issuer_key_file;config.verifier.binary='../other';await fs.writeFile(path.join(root,'native.json'),JSON.stringify(config));await stamp();
  await assert.rejects(bundle.inspectBundle(root,'linux-x64'),/account configuration/);
});
test('runtime inventory rejects symlinks outside its root',{skip:process.platform==='win32'},async t=>{
  const {root}=await fixture(t);await fs.symlink(os.tmpdir(),path.join(root,'outside'));
  await assert.rejects(bundle.inventory(root),/escapes its bundle/);
});
test('standalone Mac bundle copies retain both provided verifier architectures',async t=>{
  const source=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-universal-'));
  const resources=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-resources-'));
  t.after(()=>fs.rm(source,{recursive:true,force:true}));t.after(()=>fs.rm(resources,{recursive:true,force:true}));
  for(const target of ['darwin-arm64','darwin-x64']) {const {root}=await fixture(t,target);await fs.rename(root,path.join(source,target));}
  await bundle.copyBundles(resources,source,{platform:'darwin',architecture:'arm64',signed:false});
  for(const target of ['darwin-arm64','darwin-x64']) await bundle.inspectBundle(path.join(resources,'direct',target),target);
});

async function packagedMac(t) {
  const source=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-final-source-'));
  const resources=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-final-resources-'));
  t.after(()=>fs.rm(source,{recursive:true,force:true}));t.after(()=>fs.rm(resources,{recursive:true,force:true}));
  for(const target of ['darwin-arm64','darwin-x64']) {const {root}=await fixture(t,target);await fs.rename(root,path.join(source,target));}
  await bundle.copyBundles(resources,source,{platform:'darwin',architecture:'x64',signed:false});
  return {source,resources,options:{platform:'darwin',architecture:'universal',signed:false}};
}

test('the final universal hook verifies both copied runtimes without replacing bytes',async t=>{
  const {source,resources,options}=await packagedMac(t);
  const before=await bundle.inventory(resources);
  await bundle.verifyBundles(resources,source,options);
  await bundle.verifyBundles(resources,source,options);
  assert.deepEqual(await bundle.inventory(resources),before);
  await assert.rejects(bundle.copyBundles(resources,source,options),/Target already exists/);
});

for(const attack of ['changed','extra','missing','different-valid-input','symlink-root','unsigned']) {
  test(`final universal validation rejects ${attack} without repairing the payload`,{skip:process.platform==='win32'&&attack==='symlink-root'},async t=>{
    const {source,resources,options}=await packagedMac(t);
    const target=path.join(resources,'direct','darwin-arm64');
    if(attack==='changed') await fs.appendFile(path.join(target,'runtime/axiom-direct-verify'),'changed');
    if(attack==='extra') await fs.writeFile(path.join(target,'unexpected'),'extra');
    if(attack==='missing') await fs.rm(target,{recursive:true});
    if(attack==='different-valid-input') {
      const file=path.join(target,'runtime/axiom-direct-verify');
      await fs.appendFile(file,'different but valid executable');
      const config=JSON.parse(await fs.readFile(path.join(target,'native.json'),'utf8'));
      config.verifier.binary_sha256=bundle.digest(await fs.readFile(file));
      await fs.writeFile(path.join(target,'native.json'),JSON.stringify(config));
      const manifest=JSON.parse(await fs.readFile(path.join(target,'bundle.json'),'utf8'));
      manifest.files=await bundle.inventory(target);await fs.writeFile(path.join(target,'bundle.json'),JSON.stringify(manifest));
      await bundle.inspectBundle(target,'darwin-arm64');
    }
    if(attack==='symlink-root') {await fs.rm(target,{recursive:true});await fs.symlink(path.join(source,'darwin-arm64'),target,'dir');}
    if(attack==='unsigned') Object.assign(options,{signed:true,teamId:'ABCDEFGHIJ'});
    await assert.rejects(bundle.verifyBundles(resources,source,options));
    if(attack==='missing') await assert.rejects(fs.stat(target),{code:'ENOENT'});
    if(attack==='extra') assert.equal(await fs.readFile(path.join(target,'unexpected'),'utf8'),'extra');
  });
}

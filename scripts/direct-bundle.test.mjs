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
test('each thin macOS packaging hook retains both verifier architectures for universal merge',async t=>{
  const source=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-universal-'));
  const resources=await fs.mkdtemp(path.join(os.tmpdir(),'axiom-direct-resources-'));
  t.after(()=>fs.rm(source,{recursive:true,force:true}));t.after(()=>fs.rm(resources,{recursive:true,force:true}));
  for(const target of ['darwin-arm64','darwin-x64']) {const {root}=await fixture(t,target);await fs.rename(root,path.join(source,target));}
  await bundle.copyBundles(resources,source,{platform:'darwin',architecture:'arm64',signed:false});
  for(const target of ['darwin-arm64','darwin-x64']) await bundle.inspectBundle(path.join(resources,'direct',target),target);
});

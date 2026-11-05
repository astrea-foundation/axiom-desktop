import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {mkdtemp,rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
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

test('thin Mac hooks copy runtimes and the final universal hook only revalidates them', async () => {
  const directory=await mkdtemp(path.join(os.tmpdir(),'axiom-mac-hook-'));
  const direct=require('../../../scripts/direct-bundle.cjs');
  const chutesPath=require.resolve('../scripts/bundle-chutes-runtime.cjs');
  const licensesPath=require.resolve('../../../scripts/package-licenses.cjs');
  const originalChutes=require(chutesPath);
  const originalLicenses=require(licensesPath);
  const originalCopy=direct.copyBundles,originalVerify=direct.verifyBundles;
  const previous={direct:process.env.AXIOM_DIRECT_BUNDLE,chutes:process.env.AXIOM_CHUTES_BUNDLE};
  const calls:string[]=[];
  const record=(name:string)=>async (_resources:string,_source:string,options:{architecture:string,signed:boolean})=>{
    assert.equal(options.signed,true);
    calls.push(name+' '+options.architecture);
  };
  try {
    direct.copyBundles=record('copy direct');direct.verifyBundles=record('verify direct');
    require.cache[chutesPath]!.exports=Object.assign(record('copy chutes'),{verifyPackagedBundle:record('verify chutes')});
    require.cache[licensesPath]!.exports=async()=>{};
    process.env.AXIOM_DIRECT_BUNDLE=directory;process.env.AXIOM_CHUTES_BUNDLE=directory;
    const context={packager:{projectDir:path.join(directory,'apps','desktop'),getResourcesDir:()=>path.join(directory,'resources'),config:{forceCodeSigning:true,extraMetadata:{axiomUpdateFormat:'pkg'}}},electronPlatformName:'darwin',arch:1,appOutDir:directory};
    const afterPack=require('../scripts/after-pack.cjs');
    for(const arch of [1,3,4]) {context.arch=arch;await afterPack(context);}
    assert.deepEqual(calls,['copy direct x64','copy chutes x64','copy direct arm64','copy chutes arm64','verify direct universal','verify chutes universal']);
    direct.verifyBundles=async()=>{throw new Error('merged verifier inventory rejected');};
    await assert.rejects(afterPack(context),/merged verifier inventory rejected/);
    direct.verifyBundles=record('verify direct');
    require.cache[chutesPath]!.exports.verifyPackagedBundle=async()=>{throw new Error('merged helper signature rejected');};
    await assert.rejects(afterPack(context),/merged helper signature rejected/);
  } finally {
    direct.copyBundles=originalCopy;direct.verifyBundles=originalVerify;
    require.cache[chutesPath]!.exports=originalChutes;require.cache[licensesPath]!.exports=originalLicenses;
    if(previous.direct===undefined)delete process.env.AXIOM_DIRECT_BUNDLE;else process.env.AXIOM_DIRECT_BUNDLE=previous.direct;
    if(previous.chutes===undefined)delete process.env.AXIOM_CHUTES_BUNDLE;else process.env.AXIOM_CHUTES_BUNDLE=previous.chutes;
    await rm(directory,{recursive:true,force:true});
  }
});

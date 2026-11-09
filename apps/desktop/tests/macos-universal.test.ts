import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {mkdir,mkdtemp,rm} from 'node:fs/promises';
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

test('universal temporary apps defer frozen runtimes while final and ordinary thin apps copy strictly', async () => {
  const directory=await mkdtemp(path.join(os.tmpdir(),'axiom-mac-hook-'));
  const direct=require('../../../scripts/direct-bundle.cjs');
  const licensesPath=require.resolve('../../../scripts/package-licenses.cjs');
  const originalLicenses=require(licensesPath);
  const originalCopy=direct.copyBundles;
  const previous=process.env.AXIOM_DIRECT_BUNDLE;
  const calls:string[]=[];
  const record=(name:string)=>async (_resources:string,_source:string,options:{architecture:string,signed:boolean})=>{
    assert.equal(options.signed,true);calls.push(name+' '+options.architecture);
  };
  try {
    direct.copyBundles=record('copy direct');
    require.cache[licensesPath]!.exports=async()=>{};
    process.env.AXIOM_DIRECT_BUNDLE=directory;
    const options=createRequire(require.resolve('electron-builder'))('./builder.js').normalizeOptions({mac:['pkg'],universal:true,x64:true});
    const {Platform}=builder('./index.js');
    const {PlatformPackager}=builder('./platformPackager.js');
    const packager={projectDir:path.join(directory,'apps','desktop'),platform:Platform.MAC,packagerOptions:options,platformSpecificBuildOptions:{},computeAppOutDir:PlatformPackager.prototype.computeAppOutDir,
      getResourcesDir:(app:string)=>path.join(app,'Axiom.app','Contents','Resources'),config:{forceCodeSigning:true,extraMetadata:{axiomUpdateFormat:'pkg'}}};
    const outDir=path.join(directory,'dist'),universal=packager.computeAppOutDir(outDir,4);
    const context={packager,electronPlatformName:'darwin',arch:1,appOutDir:universal+'-x64-temp',outDir};
    const afterPack=require('../scripts/after-pack.cjs');
    await afterPack(context);
    context.arch=3;context.appOutDir=universal+'-arm64-temp';await afterPack(context);
    assert.deepEqual(calls,[],'frozen frameworks must never enter the universal merger');
    context.arch=4;context.appOutDir=universal;await afterPack(context);
    assert.deepEqual(calls,['copy direct universal']);
    context.arch=1;context.appOutDir=packager.computeAppOutDir(outDir,1);await afterPack(context);
    assert.deepEqual(calls.slice(-1),['copy direct x64'],'a mixed x64/universal command still includes runtimes in its ordinary x64 app');
    context.arch=4;context.appOutDir=universal;
    direct.copyBundles=async()=>{throw new Error('verifier inventory rejected');};
    await assert.rejects(afterPack(context),/verifier inventory rejected/);
    direct.copyBundles=record('copy direct');
    context.arch=1;context.appOutDir=universal+'-x64-temp';
    await mkdir(path.join(packager.getResourcesDir(context.appOutDir),'direct'),{recursive:true});
    await assert.rejects(afterPack(context),/Frozen runtimes must be added after the universal merge/);
  } finally {
    direct.copyBundles=originalCopy;require.cache[licensesPath]!.exports=originalLicenses;
    if(previous===undefined)delete process.env.AXIOM_DIRECT_BUNDLE;else process.env.AXIOM_DIRECT_BUNDLE=previous;
    await rm(directory,{recursive:true,force:true});
  }
});

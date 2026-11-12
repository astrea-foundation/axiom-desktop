import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import bundle from './direct-bundle.cjs';

const root=fileURLToPath(new URL('..',import.meta.url));
const args=process.argv.slice(2), values=new Map();
for(let i=0;i<args.length;i+=2) {
  if(!['--output','--codesign-identity','--windows-sign-script'].includes(args[i]) || !args[i+1] || values.has(args[i])) throw new Error('Invalid AstreaAI bundle arguments');
  values.set(args[i],args[i+1]);
}
if(!values.has('--output')) throw new Error('Choose an AstreaAI output directory');
function run(program,args,options={}) {
  const r=spawnSync(program,args,{stdio:'inherit',...options});
  if(r.error || r.status!==0) throw new Error('AstreaAI verifier build failed');
  return r;
}
const target=process.platform+'-'+process.arch;
const rustTarget={'darwin-arm64':'aarch64-apple-darwin','darwin-x64':'x86_64-apple-darwin','win32-x64':'x86_64-pc-windows-msvc','win32-arm64':'aarch64-pc-windows-msvc','linux-x64':'x86_64-unknown-linux-gnu'}[target];
if(!['darwin-arm64','darwin-x64','win32-x64','win32-arm64','linux-x64'].includes(target)) throw new Error('Unsupported AstreaAI build target');
const revision=run('git',['rev-parse','HEAD'],{stdio:'pipe',encoding:'utf8'}).stdout.trim();
if(run('git',['status','--porcelain'],{stdio:'pipe',encoding:'utf8'}).stdout.trim()) throw new Error('Commit the verifier packaging source before building');
const source=path.join(root,'vendor/axiom-direct-verifier');
const provenance=JSON.parse(await fs.readFile(path.join(source,'AXIOM_SOURCE.json'),'utf8'));
if(provenance.schema!==1 || provenance.canonical_repository!=='https://github.com/astrea-foundation/axiom-inference') throw new Error('Unexpected AstreaAI verifier provenance');
for(const [name,hash] of Object.entries(provenance.files)) {
  if(path.isAbsolute(name) || name.includes('\\') || name.split('/').some(x=>!x||x==='.'||x==='..')) throw new Error('Invalid verifier source path');
  const file=path.join(source,name), st=await fs.lstat(file);
  if(!st.isFile() || st.isSymbolicLink() || bundle.digest(await fs.readFile(file))!==hash) throw new Error('Verifier source inventory mismatch');
}
const output=path.join(path.resolve(values.get('--output')),target);
try {await fs.lstat(output); throw new Error('AstreaAI output already exists');} catch(e) {if(e.code!=='ENOENT') throw e;}
const build=path.join(root,'.local/direct-build',target);
await fs.mkdir(build,{recursive:true});
const environment={...process.env,CARGO_TARGET_DIR:process.env.CARGO_TARGET_DIR||path.join(build,'target'),CARGO_BUILD_JOBS:process.env.CARGO_BUILD_JOBS||'2'};
run('rustup',['target','add',rustTarget]);
run('cargo',['build','--target',rustTarget,'--manifest-path',path.join(source,'Cargo.toml'),'--locked','--release','--bin','axiom-direct-verify'],{env:environment});
await fs.mkdir(path.join(output,'runtime'),{recursive:true});
const binaryName='axiom-direct-verify'+(process.platform==='win32'?'.exe':'');
await fs.copyFile(path.join(environment.CARGO_TARGET_DIR,rustTarget,'release',binaryName),path.join(output,'runtime',binaryName));
if(process.platform!=='win32') await fs.chmod(path.join(output,'runtime',binaryName),0o755);
if(values.has('--codesign-identity')) {
  if(process.platform!=='darwin') throw new Error('Apple signing requires macOS');
  run('codesign',['--force','--options','runtime','--timestamp','--sign',values.get('--codesign-identity'),path.join(output,'runtime',binaryName)]);
}
if(values.has('--windows-sign-script')) {
  if(process.platform!=='win32') throw new Error('Windows signing requires Windows');
  for(const entry of await bundle.inventory(output)) if(!entry.symlink && /\.(exe|dll|pyd)$/i.test(entry.path)) run('pwsh',['-NoProfile','-NonInteractive','-File',path.resolve(values.get('--windows-sign-script')),'-FilePath',path.join(output,entry.path)]);
}
const config=JSON.parse(await fs.readFile(path.join(root,'deploy/astreaai-production.json'),'utf8'));
config.verifier.binary='runtime/axiom-direct-verify'+(process.platform==='win32'?'.exe':'');
const executable=path.join(output,config.verifier.binary);
config.verifier.binary_sha256=bundle.digest(await fs.readFile(executable));
const clean=process.platform==='win32'?Object.fromEntries(['SystemRoot','WINDIR','USERPROFILE','LOCALAPPDATA','TEMP','TMP'].filter(k=>process.env[k]).map(k=>[k,process.env[k]])):{PATH:'/usr/bin:/bin:/usr/sbin:/sbin'};
if(process.platform==='win32') clean.PATH=path.join(process.env.SystemRoot,'System32');
const selfTest=spawnSync(executable,['--self-test'],{env:clean,encoding:'utf8'});
if(selfTest.error || selfTest.status!==0) throw new Error('Native AstreaAI self test failed: '+JSON.stringify({code:selfTest.error?.code,status:selfTest.status,signal:selfTest.signal,stderr:(selfTest.stderr||'').slice(0,2000)}));
const check=JSON.parse(selfTest.stdout);
if(check.passed!==true || check.native!==true) throw new Error('Native AstreaAI self test failed');
const invalid=spawnSync(executable,[],{env:clean,input:'{}',encoding:'utf8'});
if(invalid.status===0 || invalid.stdout || invalid.stderr.trim()!=='direct workload attestation rejected') throw new Error('Native verifier rejection protocol failed');
await fs.writeFile(path.join(output,'native.json'),JSON.stringify(config,null,2)+'\n');
await fs.copyFile(path.join(source,'LICENSE'),path.join(output,'LICENSE'));
await fs.copyFile(path.join(source,'AXIOM_SOURCE.json'),path.join(output,'AXIOM_SOURCE.json'));
// Record the exact locked native dependency licenses in the distributable.
const metadata=JSON.parse(run('cargo',['metadata','--manifest-path',path.join(source,'Cargo.toml'),'--locked','--format-version','1'],{env:environment,stdio:'pipe',encoding:'utf8',maxBuffer:16*1024*1024}).stdout);
const licenses=path.join(output,'licenses');await fs.mkdir(licenses,{recursive:true});
await fs.writeFile(path.join(licenses,'dependencies.json'),JSON.stringify(metadata.packages.map(p=>({name:p.name,version:p.version,license:p.license,source:p.source})),null,2)+'\n');
for(const item of metadata.packages) {
  const directory=path.dirname(item.manifest_path);
  for(const entry of await fs.readdir(directory,{withFileTypes:true})) if(entry.isFile() && /^(LICENSE|COPYING|NOTICE)/i.test(entry.name)) {
    const destination=path.join(licenses,item.name+'-'+item.version);await fs.mkdir(destination,{recursive:true});
    await fs.copyFile(path.join(directory,entry.name),path.join(destination,entry.name));
  }
}
const manifest={schemaVersion:1,provider:'axiom-direct',target,nativeRevision:revision,nativeClean:true,sourceInventorySHA256:bundle.digest(await fs.readFile(path.join(source,'AXIOM_SOURCE.json'))),runtimeLockSHA256:bundle.digest(await fs.readFile(path.join(source,'Cargo.lock'))),buildLockSHA256:bundle.digest(await fs.readFile(path.join(source,'Cargo.toml'))),files:await bundle.inventory(output)};
await fs.writeFile(path.join(output,'bundle.json'),JSON.stringify(manifest,null,2)+'\n');
await bundle.inspectBundle(output,target,{signed:values.has('--codesign-identity')||values.has('--windows-sign-script'),teamId:process.env.APPLE_TEAM_ID});
console.log(JSON.stringify({provider:'AstreaAI',target,native:true,verifierSHA256:config.verifier.binary_sha256,output}));

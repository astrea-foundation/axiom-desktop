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
if(!['darwin-arm64','darwin-x64','win32-x64','linux-x64'].includes(target)) throw new Error('Unsupported AstreaAI build target');
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
const build=path.join(root,'.local/direct-build',target), environment=path.join(build,'environment');
await fs.mkdir(build,{recursive:true});
const selected=process.env.AXIOM_HELPER_PYTHON || '3.12';
run('uv',['venv','--clear','--python',selected,environment]);
const python=path.join(environment,process.platform==='win32'?'Scripts/python.exe':'bin/python');
run('uv',['pip','install','--python',python,'--only-binary',':all:','--require-hashes','-r',path.join(source,'deploy/direct/requirements.lock'),'-r',path.join(root,'scripts/direct-bundle-build.lock')]);
run('uv',['pip','check','--python',python]);
const command=['-m','PyInstaller','--noconfirm','--clean','--onedir','--name','axiom-direct-verify','--paths',path.join(source,'providers/direct'),'--collect-submodules','anyio','--collect-data','certifi','--distpath',path.join(build,'dist'),'--workpath',path.join(build,'work'),'--specpath',build];
if(values.has('--codesign-identity')) command.push('--codesign-identity',values.get('--codesign-identity'));
command.push(path.join(root,'scripts/direct-verifier-entry.py'));run(python,command);
const runtime=path.join(build,'dist/axiom-direct-verify');
await fs.mkdir(output,{recursive:true});
await fs.cp(runtime,path.join(output,'runtime'),{recursive:true,verbatimSymlinks:true});
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
const check=JSON.parse(run(executable,['--self-test'],{env:clean,stdio:'pipe',encoding:'utf8'}).stdout);
if(check.passed!==true || check.frozen!==true) throw new Error('Frozen AstreaAI self test failed');
const invalid=spawnSync(executable,[],{env:clean,input:'{}',encoding:'utf8'});
if(invalid.status===0 || invalid.stdout || invalid.stderr.trim()!=='direct workload attestation rejected') throw new Error('Frozen verifier rejection protocol failed');
await fs.writeFile(path.join(output,'native.json'),JSON.stringify(config,null,2)+'\n');
await fs.copyFile(path.join(source,'LICENSE'),path.join(output,'LICENSE'));
await fs.copyFile(path.join(source,'AXIOM_SOURCE.json'),path.join(output,'AXIOM_SOURCE.json'));
run(python,['-c',
  'import importlib.metadata as m,pathlib,shutil,sys,re,hashlib; out=pathlib.Path(sys.argv[1]); '+
  '\nfor d in m.distributions():\n for f in d.files or []:\n  if "license" in str(f).lower() and d.locate_file(f).is_file():\n   name=re.sub("[^A-Za-z0-9_.-]","_",d.metadata["Name"]+"-"+d.version); filename=hashlib.sha256(str(f).encode()).hexdigest()[:12]+"-"+pathlib.Path(f).name; p=out/name/filename; p.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(d.locate_file(f),p)',
  path.join(output,'licenses')]);
const manifest={schemaVersion:1,provider:'axiom-direct',target,nativeRevision:revision,nativeClean:true,sourceInventorySHA256:bundle.digest(await fs.readFile(path.join(source,'AXIOM_SOURCE.json'))),runtimeLockSHA256:bundle.digest(await fs.readFile(path.join(source,'deploy/direct/requirements.lock'))),buildLockSHA256:bundle.digest(await fs.readFile(path.join(root,'scripts/direct-bundle-build.lock'))),files:await bundle.inventory(output)};
await fs.writeFile(path.join(output,'bundle.json'),JSON.stringify(manifest,null,2)+'\n');
await bundle.inspectBundle(output,target,{signed:values.has('--codesign-identity')||values.has('--windows-sign-script'),teamId:process.env.APPLE_TEAM_ID});
console.log(JSON.stringify({provider:'AstreaAI',target,frozen:true,verifierSHA256:config.verifier.binary_sha256,output}));

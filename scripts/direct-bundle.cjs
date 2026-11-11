const fs = require('node:fs/promises');
const path = require('node:path');
const {createHash} = require('node:crypto');
const {spawnSync} = require('node:child_process');

const targets = new Set(['darwin-arm64','darwin-x64','win32-x64','win32-arm64','linux-x64']);
const digest = data => createHash('sha256').update(data).digest('hex');
const inside = (root, child) => child.startsWith(root + path.sep);
const macho = data => ['cffaedfe','cefaedfe','cafebabe','cafebabf'].includes(data.subarray(0,4).toString('hex'));

async function inventory(root) {
  root = await fs.realpath(root);
  const files = [];
  async function visit(directory) {
    for (const entry of (await fs.readdir(directory,{withFileTypes:true})).sort((a,b)=>a.name.localeCompare(b.name,'en'))) {
      const file = path.join(directory,entry.name), name = path.relative(root,file).split(path.sep).join('/');
      if (name === 'bundle.json') continue;
      if (entry.isSymbolicLink()) {
        const target = await fs.readlink(file), actual = await fs.realpath(file);
        if (path.isAbsolute(target) || !inside(root,actual)) throw new Error('AstreaAI runtime symlink escapes its bundle');
        files.push({path:name,symlink:target});
      } else if (entry.isDirectory()) await visit(file);
      else if (entry.isFile()) {
        const data = await fs.readFile(file);
        files.push({path:name,bytes:data.length,sha256:digest(data)});
      } else throw new Error('Ordinary AstreaAI runtime files required');
    }
  }
  await visit(root);
  return files.sort((a,b)=>a.path.localeCompare(b.path,'en'));
}

function run(program,args,options={}) {
  const result = spawnSync(program,args,{encoding:'utf8',...options});
  if (result.error || result.status !== 0) throw new Error('AstreaAI runtime validation failed');
  return result;
}

async function inspectBundle(root,target,options={}) {
  if (!targets.has(target)) throw new Error('Unsupported AstreaAI runtime target');
  const manifest = JSON.parse(await fs.readFile(path.join(root,'bundle.json'),'utf8'));
  const config = JSON.parse(await fs.readFile(path.join(root,'native.json'),'utf8'));
  if (manifest.schemaVersion !== 1 || manifest.provider !== 'axiom-direct' || manifest.target !== target || manifest.nativeClean !== true) throw new Error('Unrecognized AstreaAI bundle');
  if (JSON.stringify(await inventory(root)) !== JSON.stringify(manifest.files)) throw new Error('AstreaAI bundle inventory mismatch');
  const executable = 'runtime/axiom-direct-verify' + (target.startsWith('win32-')?'.exe':'');
  if (config.verifier?.binary !== executable || config.issuer_key_file || config.account?.origin !== 'https://api.axiom.stream') throw new Error('Installed AstreaAI account configuration required');
  const binary = path.join(root,executable);
  if (digest(await fs.readFile(binary)) !== config.verifier.binary_sha256) throw new Error('AstreaAI verifier digest mismatch');
  require('./native-binary.cjs')(binary,{darwin:'mac',win32:'win',linux:'linux'}[target.split('-')[0]],target.split('-')[1]);
  if (options.signed && target.startsWith('darwin-')) {
    if (!options.teamId) throw new Error('Expected Apple team required for AstreaAI');
    for (const entry of manifest.files.filter(file=>!file.symlink && file.path.startsWith('runtime/'))) {
      const file=path.join(root,entry.path);
      if (!macho(await fs.readFile(file))) continue;
      run('codesign',['--verify','--strict',file]);
      const metadata=run('codesign',['-dv','--verbose=4',file]).stderr;
      if (!metadata.split('\n').includes('TeamIdentifier='+options.teamId)) throw new Error('AstreaAI signing team mismatch');
    }
  }
  if (options.signed && target.startsWith('win32-')) {
    const check=path.join(__dirname,'verify-direct-signature.ps1');
    for (const entry of manifest.files.filter(file=>!file.symlink && /\.(exe|dll|pyd)$/i.test(file.path))) run('pwsh',['-NoProfile','-NonInteractive','-File',check,'-FilePath',path.join(root,entry.path)]);
  }
  return {manifest,config,binary};
}

async function bundleTargets(source,options) {
  const darwinTargets = ['darwin-arm64','darwin-x64'];
  const hasBothMacRuntimes = options.platform === 'darwin' && (await Promise.all(darwinTargets.map(async target => {
    try { return (await fs.stat(path.join(source,target))).isDirectory(); }
    catch(error) { if(error.code==='ENOENT') return false; throw error; }
  }))).every(Boolean);
  // Electron's universal packager invokes hooks for each thin app before merge.
  // Give both apps identical signed resources so the final app retains both CPUs.
  let architecture = options.architecture;
  if (options.platform === 'win32' && architecture === 'arm64') {
    try { await fs.access(path.join(source,'win32-arm64','bundle.json')); }
    catch (error) { if(error.code!=='ENOENT') throw error; architecture='x64'; }
  }
  return options.platform === 'darwin'  && (options.architecture === 'universal' || hasBothMacRuntimes)
    ? ['darwin-arm64','darwin-x64']
    : [options.platform+'-'+architecture];
}

async function verifyBundles(resources,source,options) {
  for (const target of await bundleTargets(source,options)) {
    const input=path.join(source,target), output=path.join(resources,'direct',target);
    if (!(await fs.lstat(output)).isDirectory()) throw new Error('Packaged AstreaAI runtime must be an ordinary directory');
    await inspectBundle(input,target,options);
    await inspectBundle(output,target,options);
    if (!(await fs.readFile(path.join(input,'bundle.json'))).equals(await fs.readFile(path.join(output,'bundle.json')))) {
      throw new Error('Packaged AstreaAI runtime differs from its validated input');
    }
  }
}

async function copyBundles(resources,source,options) {
  for (const target of await bundleTargets(source,options)) {
    const input=path.join(source,target), output=path.join(resources,'direct',target);
    await inspectBundle(input,target,options);
    await fs.mkdir(path.dirname(output),{recursive:true});
    await fs.cp(input,output,{recursive:true,verbatimSymlinks:true,errorOnExist:true,force:false});
    await inspectBundle(output,target,options);
  }
}

async function signMacBundles(source,output,identity,teamId) {
  if (process.platform !== 'darwin' || !identity || !teamId) throw new Error('Native Apple signing identity required');
  for (const target of ['darwin-arm64','darwin-x64']) {
    const input=path.join(source,target), destination=path.join(output,target);
    const {manifest,config}=await inspectBundle(input,target);
    await fs.mkdir(path.dirname(destination),{recursive:true});
    await fs.cp(input,destination,{recursive:true,verbatimSymlinks:true,errorOnExist:true,force:false});
    const command=['--force','--options','runtime','--timestamp','--sign',identity];
    for (const entry of manifest.files.filter(file=>!file.symlink && file.path.startsWith('runtime/'))) {
      const file=path.join(destination,entry.path);
      if (macho(await fs.readFile(file))) run('codesign',[...command,file]);
    }
    async function frameworks(directory) {
      const result=[];
      for (const entry of await fs.readdir(directory,{withFileTypes:true})) if(entry.isDirectory()) {
        const file=path.join(directory,entry.name); result.push(...await frameworks(file));
        if (entry.name.endsWith('.framework')) result.push(file);
      }
      return result;
    }
    for (const file of await frameworks(path.join(destination,'runtime'))) run('codesign',[...command,file]);
    config.verifier.binary_sha256=digest(await fs.readFile(path.join(destination,config.verifier.binary)));
    await fs.writeFile(path.join(destination,'native.json'),JSON.stringify(config,null,2)+'\n');
    manifest.files=await inventory(destination);
    await fs.writeFile(path.join(destination,'bundle.json'),JSON.stringify(manifest,null,2)+'\n');
    await inspectBundle(destination,target,{signed:true,teamId});
  }
}

module.exports={inventory,inspectBundle,copyBundles,verifyBundles,signMacBundles,digest};

import {createHash} from 'node:crypto';
import {readFile, mkdir} from 'node:fs/promises';
import {spawnSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const root = fileURLToPath(new URL('..',import.meta.url));
const source = path.join(root,'vendor/axiom-chutes-helper');
const args = process.argv.slice(2);
const values = new Map();
for (let index=0;index<args.length;index+=2) {
  if (!['--output','--channel','--codesign-identity','--windows-sign-script'].includes(args[index]) || !args[index+1] || values.has(args[index])) throw new Error('Invalid Chutes bundle build arguments');
  values.set(args[index],args[index+1]);
}
if (!values.has('--output')) throw new Error('Choose a new --output directory');
const channel = values.get('--channel') || 'production';
if (!['staging','production'].includes(channel)) throw new Error('Invalid helper release channel');
const inventory = JSON.parse(await readFile(path.join(source,'AXIOM_SOURCE.json'),'utf8'));
if (inventory.canonical_repository !== 'https://github.com/astrea-foundation/axiom-inference') throw new Error('Unexpected helper source authority');
for (const [name,expected] of Object.entries(inventory.files)) {
  if (name.includes('\\') || path.isAbsolute(name) || name.split('/').some(part=>!part || part==='.' || part==='..')) throw new Error('Invalid helper source path');
  const actual = createHash('sha256').update(await readFile(path.join(source,name))).digest('hex');
  if (actual !== expected) throw new Error(`Vendored helper source differs: ${name}`);
}
const authorityPath = path.join(source,`deploy/chutes/client-authority.${channel}.json`);
const authority = JSON.parse(await readFile(authorityPath,'utf8'));
if (authority.channel !== channel) throw new Error('Reviewed helper authority differs from the selected release channel');
const defaults = JSON.parse(await readFile(path.join(source,'deploy/chutes/client-defaults.json'),'utf8'));
const output = path.resolve(values.get('--output'));
const environment = path.join(path.dirname(output),'helper-build-environment');
const project = path.join(source,'providers/chutes');
function run(program,arguments_,options={}) {
  const result = spawnSync(program,arguments_,{stdio:'inherit',...options});
  if (result.error || result.status !== 0) throw result.error || new Error(`${program} failed (${result.status})`);
  return result;
}
await mkdir(path.dirname(output),{recursive:true});
const selectedPython = process.env.AXIOM_HELPER_PYTHON
  ? run(process.env.AXIOM_HELPER_PYTHON,['-c','import sys;print(sys.executable)'],{stdio:'pipe',encoding:'utf8'}).stdout.trim()
  : run('uv',['python','find','3.12'],{stdio:'pipe',encoding:'utf8'}).stdout.trim();
run('uv',['sync','--project',project,'--locked','--no-default-groups','--group','bundle','--group','dev','--python',selectedPython],{env:{...process.env,UV_PROJECT_ENVIRONMENT:environment}});
const python = path.join(environment,process.platform==='win32'?'Scripts/python.exe':'bin/python');
run(python,['-m','pytest','-q',path.join(project,'tests/test_private_state.py')],{cwd:source,env:{...process.env,PYTHONPATH:project}});
const command = [path.join(source,'scripts/build_chutes_helper.py'),'--authority',authorityPath,
  '--chute-id',defaults.chute_id,'--maximum-charge-microusd',String(defaults.maximum_charge_microusd),'--output',output];
for (const flag of ['--codesign-identity','--windows-sign-script']) if (values.has(flag)) command.push(flag,values.get(flag));
run(python,command);
const bundle = JSON.parse(await readFile(path.join(output,'bundle.json'),'utf8'));
await (await import('../apps/desktop/scripts/bundle-chutes-runtime.cjs')).default.validateBundle(output,{
  platform:bundle.platform, architecture:bundle.architecture,channel,
  signed:values.has('--codesign-identity') || values.has('--windows-sign-script'),teamId:process.env.APPLE_TEAM_ID,
});

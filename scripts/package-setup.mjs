import {execFileSync} from 'node:child_process';
import {copyFile, mkdir, readFile, writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import path from 'node:path';
import {createRequire} from 'node:module';

const require = createRequire(import.meta.url);
const args = process.argv.slice(2), unsigned = args[0] === '--unsigned', buildOnly = args[0] === '--build-only', input = args[0] === '--input' ? args[1] : null;
if (process.platform !== 'win32' || process.arch !== 'x64' || (args.length && !(args.length === 1 && (unsigned || buildOnly)) && !(args.length === 2 && input))) throw new Error('Usage on x64 Windows: node scripts/package-setup.mjs [--unsigned|--build-only|--input DIRECTORY]');
const version = (await readFile('apps/axiom-setup/Cargo.toml','utf8')).match(/^version = "([^"]+)"/m)?.[1];
const revision = execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();
const publisher = process.env.AXIOM_SIGNING_PUBLISHER || 'Astrea Labs, Inc.';
if (!unsigned && !process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('Setup needs compiled update trust');
// A single static x64 binary also runs on supported ARM64 Windows via emulation;
// the verified combined NSIS installer chooses the native application payload.
const env = {...process.env,RUSTFLAGS:'-C target-feature=+crt-static',
  AXIOM_UPDATE_PUBLIC_KEYS:unsigned ? '' : process.env.AXIOM_UPDATE_PUBLIC_KEYS, AXIOM_SIGNING_PUBLISHER:publisher};
if (!input) execFileSync('cargo',['build','--locked','--release','-p','axiom-setup','--target','x86_64-pc-windows-msvc'],{env,stdio:'inherit'});
const output = 'apps/axiom-setup/dist';
await mkdir(output,{recursive:true});
const name = `AxiomSetup-${version}-${revision.slice(0,12)}.exe`, file = path.join(output,name);
const source = input ? path.join(input,name) : 'target/x86_64-pc-windows-msvc/release/axiom-setup.exe';
const bytes = await readFile(source);
const receipt = {schemaVersion:1,version,revision,bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex'),trustedKeys:env.AXIOM_UPDATE_PUBLIC_KEYS,publisher};
if (input && JSON.stringify(JSON.parse(await readFile(path.join(input,'setup-input.json'),'utf8'))) !== JSON.stringify(receipt)) throw new Error('Setup build input does not match this revision, trust configuration and bytes');
await copyFile(source,file);
require('./native-binary.cjs')(file,'win','x64');
if (!unsigned && !buildOnly) require('../apps/desktop/scripts/windows-signing.cjs').sign({path:path.resolve(file),hash:'sha256',isNest:false});
execFileSync('pwsh',['-NoProfile','-NonInteractive','-File','apps/axiom-setup/scripts/verify-setup.ps1','-FilePath',file,'-Version',version,...(unsigned || buildOnly ? [] : ['-Publisher',publisher])],{stdio:'inherit',env});
if (buildOnly) await writeFile(path.join(output,'setup-input.json'),JSON.stringify(receipt)+'\n');
console.log(`Prepared ${unsigned || buildOnly ? 'unsigned input' : 'signed'} Windows setup: ${file}`);

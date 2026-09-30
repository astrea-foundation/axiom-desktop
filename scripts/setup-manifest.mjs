import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {readFile,writeFile} from 'node:fs/promises';
import path from 'node:path';
import {parseSetup,signSetup} from '../packages/desktop-releases/windows-setup.mjs';

const [directory,releaseVersion,signing='unsigned'] = process.argv.slice(2);
if (!directory || !releaseVersion || !['signed','unsigned'].includes(signing)) throw new Error('Usage: setup-manifest.mjs DIRECTORY RELEASE_VERSION [signed|unsigned]');
const version=(await readFile('apps/axiom-setup/Cargo.toml','utf8')).match(/^version = "([^"]+)"/m)?.[1];
const revision=process.env.AXIOM_RELEASE_REVISION ?? execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();
const sequence=Number(process.env.AXIOM_RELEASE_SEQUENCE ?? execFileSync('git',['show','-s','--format=%ct',revision],{encoding:'utf8'}).trim());
const name=`AxiomSetup-${version}-${revision.slice(0,12)}.exe`, bytes=await readFile(path.join(directory,name));
let metadata=parseSetup({schemaVersion:1,version,releaseVersion,revision,sequence,publisher:process.env.AXIOM_SIGNING_PUBLISHER ?? 'Astrea Labs, Inc.',signing:'unsigned',
  artifact:{name,bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex'),url:`https://cdn.axiom.stream/downloads/setup/${revision}/${name}`,githubUrl:`https://github.com/astrea-foundation/axiom-releases/releases/download/v${releaseVersion}/${name}`}});
if(signing==='signed')metadata=signSetup(metadata,process.env.AXIOM_UPDATE_SIGNING_KEY,process.env.AXIOM_UPDATE_PUBLIC_KEYS ?? '');
await writeFile(path.join(directory,'windows-setup.json'),JSON.stringify(metadata,null,2)+'\n');
console.log(`Prepared ${signing} setup inventory for ${name}`);

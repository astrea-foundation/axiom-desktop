// Native owns final signed bytes. The platform publisher activates the draft
// only after verifying those exact bytes at both public download destinations.
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import path from 'node:path';
import {requireCompleteRelease} from '../packages/desktop-releases/manifest.mjs';
import {verifyRelease} from '../packages/desktop-releases/signing.mjs';
const directory = process.argv[2];
if (!directory) throw new Error('Usage: publish-github-release.mjs DIRECTORY');
const manifest = requireCompleteRelease(JSON.parse(await readFile(path.join(directory,'manifest.json'),'utf8')));
const keys = (process.env.AXIOM_UPDATE_PUBLIC_KEYS ?? '').split(',');
if (!keys.some(key=>{try {verifyRelease(manifest,key.trim()); return true;} catch {return false;}})) throw new Error('Untrusted release');
const tag = `v${manifest.version}`, repository = 'astrea-foundation/axiom-releases';
const gh = args => execFileSync('gh', args, {encoding:'utf8', maxBuffer:16*1024*1024});
const git = args => execFileSync('git',args,{encoding:'utf8'}).trim();
if (git(['rev-parse',`${tag}^{commit}`]) !== manifest.revision) throw new Error('Tag does not match signed inventory');
git(['merge-base','--is-ancestor',manifest.revision,'origin/main']);
// Authenticate every local artifact before making any mutation.
const files = [...manifest.downloads,...manifest.cliDownloads];
for (const file of files) {
  const bytes = await readFile(path.join(directory,file.name));
  if (bytes.length !== file.bytes || createHash('sha256').update(bytes).digest('hex') !== file.sha256) throw new Error(`Changed installer: ${file.name}`);
}
const repositoryInfo = JSON.parse(gh(['api', `repos/${repository}`]));
if (repositoryInfo.private || !repositoryInfo.default_branch) throw new Error('A public distribution repository with an initial README commit is required');
const response = JSON.parse(gh(['api',`repos/${repository}/releases?per_page=100`]));
let release = response.find(r=>r.tag_name === tag);
if (!release) {
  gh(['release','create',tag,'--repo',repository,'--target',repositoryInfo.default_branch,'--draft','--title',`Axiom ${manifest.version}`,'--notes',`Desktop and AxiomCLI installers. SHA-256 and the signed installer inventory are attached. Public CDN downloads: https://axiom.stream/downloads`]);
  release = JSON.parse(gh(['api',`repos/${repository}/releases/tags/${tag}`]));
}

const existing = new Map(release.assets.map(asset=>[asset.name,asset]));
for (const name of [...files.map(f=>f.name),'manifest.json','SHA256SUMS']) {
  const asset = existing.get(name), bytes = await readFile(path.join(directory,name));
  if (asset) {
    const remote = execFileSync('gh',['api',`repos/${repository}/releases/assets/${asset.id}`,'-H','Accept: application/octet-stream'],{maxBuffer:2*1024**3});
    if (!bytes.equals(remote)) throw new Error(`Immutable GitHub asset changed: ${name}`);
  } else {
    if (!release.draft) throw new Error('Cannot add assets to a published release');
    gh(['release','upload',tag,path.join(directory,name),'--repo',repository]);
  }
}
console.log(`Verified GitHub release assets for ${tag}; publication is coordinated by axiom-platform.`);

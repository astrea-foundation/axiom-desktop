// Native owns final signed bytes. The platform publisher activates the draft
// only after verifying those exact bytes at both public download destinations.
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import path from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {requireCompleteRelease} from '../packages/desktop-releases/manifest.mjs';
import {verifyRelease} from '../packages/desktop-releases/signing.mjs';
const directory = process.argv[2];
const sourceRelease = process.argv[3] === '--source';
if (!directory || process.argv.length > 4 || (process.argv[3] && !sourceRelease)) throw new Error('Usage: publish-github-release.mjs DIRECTORY [--source]');
const manifest = requireCompleteRelease(JSON.parse(await readFile(path.join(directory,'manifest.json'),'utf8')));
const keys = (process.env.AXIOM_UPDATE_PUBLIC_KEYS ?? '').split(',');
if (!keys.some(key=>{try {verifyRelease(manifest,key.trim()); return true;} catch {return false;}})) throw new Error('Untrusted release');
const tag = `v${manifest.version}`, repository = sourceRelease ? 'astrea-foundation/axiom-desktop' : 'astrea-foundation/axiom-releases';
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
if ((!sourceRelease && repositoryInfo.private) || !repositoryInfo.default_branch) throw new Error('A distribution repository with an initial commit is required; public mirrors must be public');
if (sourceRelease) {
  let ref = JSON.parse(gh(['api',`repos/${repository}/git/ref/tags/${tag}`])).object;
  while (ref.type === 'tag') ref = JSON.parse(gh(['api',`repos/${repository}/git/tags/${ref.sha}`])).object;
  if (ref.type !== 'commit' || ref.sha !== manifest.revision) throw new Error('Remote source tag does not match signed inventory');
}
const response = JSON.parse(gh(['api',`repos/${repository}/releases?per_page=100`]));
let release = response.find(r=>r.tag_name === tag);
if (!release) {
  gh(['release','create',tag,'--repo',repository,...(sourceRelease ? ['--verify-tag'] : ['--target',repositoryInfo.default_branch]),'--draft','--title',`Axiom ${manifest.version}`,'--notes',`Signed Desktop and standalone AxiomCLI installers for macOS, Windows and Linux. SHA-256 checksums and the signed installer inventory are attached.\n\n${manifest.schemaVersion === 3 ? 'macOS packages support Intel and Apple Silicon. Windows installers select x64 or ARM64 automatically.\n\n' : ''}[Downloads](https://axiom.stream/downloads) · [Public release](https://github.com/astrea-foundation/axiom-releases/releases/tag/${tag})`]);
  // The tag endpoint resolves published releases; new drafts are available to
  // the authenticated operator through the release listing instead.
  for (let attempt=0; attempt<5 && !release; attempt++) {
    if (attempt) await delay(1000);
    release = JSON.parse(gh(['api',`repos/${repository}/releases?per_page=100`])).find(r=>r.tag_name === tag);
  }
  if (!release) throw new Error('Created draft is not visible; retry to resume publication');
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
if (sourceRelease && release.draft) gh(['release','edit',tag,'--repo',repository,'--draft=false','--latest']);
console.log(sourceRelease ? `Published source repository release ${tag} with verified signed installers.` : `Verified GitHub release assets for ${tag}; public channel activation is coordinated by axiom-platform.`);

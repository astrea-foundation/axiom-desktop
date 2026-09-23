import {createHash} from 'node:crypto';
import {createReadStream} from 'node:fs';
import {readdir, stat, writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import {artifactFormat, parseRelease, requireCompleteRelease} from '../packages/desktop-releases/manifest.mjs';
import {publicKeyHex, signRelease} from '../packages/desktop-releases/signing.mjs';

const [directory, version, signing = 'unsigned'] = process.argv.slice(2);
if (!directory || !version || !['unsigned','signed'].includes(signing)) throw new Error('Usage: download-manifest.mjs DIRECTORY VERSION [unsigned|signed]');
const revision = process.env.AXIOM_RELEASE_REVISION ?? execFileSync('git', ['rev-parse', 'HEAD'], {encoding:'utf8'}).trim();
const sequence = Number(process.env.AXIOM_RELEASE_SEQUENCE ?? execFileSync('git', ['show','-s','--format=%ct',revision], {encoding:'utf8'}).trim());
const downloads = [], cliDownloads = [];
for (const name of (await readdir(directory)).sort()) {
  const format = artifactFormat(name); if (!format) continue;
  if (!name.includes(`-${version}-`)) throw new Error(`Artifact version mismatch: ${name}`);
  const file = path.join(directory, name), metadata = await stat(file);
  if (!metadata.isFile() || metadata.size === 0) throw new Error(`Invalid artifact: ${name}`);
  const hash = createHash('sha256'); for await (const chunk of createReadStream(file)) hash.update(chunk);
  const product = name.startsWith('AxiomCLI-') ? 'cli' : 'desktop';
  const platform = format === 'pkg' ? 'mac' : format === 'exe' ? 'win' : 'linux';
  const arch = /-universal[.-]/.test(name) ? 'universal' : /-arm64[.-]/.test(name) ? 'arm64' : /-(?:x64|x86_64|amd64)[.-]/.test(name) ? 'x64' : null;
  (product === 'cli' ? cliDownloads : downloads).push({name, product, platform, arch, format, bytes:metadata.size, sha256:hash.digest('hex'),
    url:`https://cdn.axiom.stream/downloads/${version}/${name}`, githubUrl:`https://github.com/astrea-foundation/axiom-releases/releases/download/v${version}/${name}`});
}
let release = parseRelease({schemaVersion:signing === 'signed' || [...downloads,...cliDownloads].some(file=>file.arch === 'universal') ? 3 : 2, version, revision, sequence, signing:'unsigned', downloads, cliDownloads});
if (signing === 'signed') {
  requireCompleteRelease(release);
  const privateKey = process.env.AXIOM_UPDATE_SIGNING_KEY;
  const publicKey = publicKeyHex(privateKey);
  if (!(process.env.AXIOM_UPDATE_PUBLIC_KEYS ?? '').split(',').map(s=>s.trim()).includes(publicKey)) throw new Error('Signing key is absent from compiled trust configuration');
  release = signRelease(release, privateKey, publicKey);
}
await writeFile(path.join(directory, 'SHA256SUMS'), [...release.downloads,...release.cliDownloads].map(f=>`${f.sha256}  ${f.name}\n`).join(''));
await writeFile(path.join(directory, 'manifest.json'), JSON.stringify(release, null, 2)+'\n');
console.log(`Prepared ${release.signing} inventory for ${downloads.length} Desktop and ${cliDownloads.length} CLI installers.`);

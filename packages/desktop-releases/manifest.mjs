export const RELEASE_API = 'https://axiom.stream/api/releases/latest';
export const VERSION_PATTERN = /^(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})$/;
export const FORMATS = { mac: ['pkg'], win: ['exe'], linux: ['AppImage', 'deb', 'pacman'] };
export const CLI_FORMATS = { mac: ['pkg'], win: ['exe'], linux: ['sh'] };
export const GITHUB_RELEASES = 'https://github.com/astrea-foundation/axiom-releases/releases';

export function compareVersions(a, b) {
  if (!VERSION_PATTERN.test(a) || !VERSION_PATTERN.test(b)) throw new Error('Invalid release version');
  const left = a.split('.').map(Number), right = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return Math.sign(left[i] - right[i]);
  return 0;
}
export function artifactFormat(name) {
  if (/\.pkg\.tar\.(xz|zst)$/.test(name)) return 'pacman';
  return /\.(AppImage|deb|dmg|pkg|zip|exe|sh)$/.exec(name)?.[1] ?? null;
}
// Recursively sorted object keys, compact UTF-8 JSON, no trailing newline.
export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`;
  return JSON.stringify(value);
}
export function unsignedManifest(value) { const {signature, ...manifest} = value; return manifest; }
export function parseRelease(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || typeof value.version !== 'string' || typeof value.revision !== 'string' || ![2, 3].includes(value.schemaVersion) || !VERSION_PATTERN.test(value.version ?? '')
    || !/^[a-f0-9]{40}$/.test(value.revision ?? '') || !['signed', 'unsigned'].includes(value.signing)
    || !Number.isSafeInteger(value.sequence) || value.sequence < 1
    || !Array.isArray(value.downloads) || !Array.isArray(value.cliDownloads)
    || !value.downloads.length || value.downloads.length + value.cliDownloads.length > 100) throw new Error('Invalid release manifest');
  const keys = ['schemaVersion', 'version', 'revision', 'sequence', 'signing', 'downloads', 'cliDownloads', 'signature'];
  if (Object.keys(value).some(key => !keys.includes(key))) throw new Error('Unknown release field');
  if (value.signing === 'signed' && (!value.signature || typeof value.signature.keyId !== 'string' || typeof value.signature.value !== 'string' || !/^[a-f0-9]{16}$/.test(value.signature.keyId ?? '')
    || !/^[A-Za-z0-9+/]{85}[AQgw]==$/.test(value.signature.value ?? '') || Object.keys(value.signature).sort().join() !== 'keyId,value')) throw new Error('Missing release signature');
  if (value.signing === 'unsigned' && value.signature !== undefined) throw new Error('Unsigned release has a signature');
  const seen = new Set();
  for (const [product, files, formats] of [['desktop', value.downloads, FORMATS], ['cli', value.cliDownloads, CLI_FORMATS]]) {
    for (const file of files) {
      if (!file || typeof file !== 'object' || Array.isArray(file) || ['name','product','platform','arch','format','sha256','url','githubUrl'].some(key => typeof file[key] !== 'string')) throw new Error('Invalid release artifact');
      const format = artifactFormat(file.name ?? '');
      const namedArch = /-universal[.-]/.test(file.name) ? 'universal' : /-arm64[.-]/.test(file.name) ? 'arm64' : /-(x64|x86_64|amd64)[.-]/.test(file.name) ? 'x64' : null;
      const key = `${product}:${file.platform}:${file.arch}:${format}`;
      if (Object.keys(file).sort().join() !== 'arch,bytes,format,githubUrl,name,platform,product,sha256,url'
        || file.product !== product || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,199}$/.test(file.name ?? '')
        || !file.name.includes(`-${value.version}-`) || !(value.schemaVersion === 3 ? (file.platform === 'linux' ? ['x64'] : ['universal']) : ['x64', 'arm64']).includes(file.arch)
        || !Object.hasOwn(formats, file.platform) || !formats[file.platform].includes(format)
        || file.format !== format || namedArch !== file.arch || seen.has(key)
        || !Number.isSafeInteger(file.bytes) || file.bytes < 1 || file.bytes > 2 * 1024 ** 3
        || !/^[a-f0-9]{64}$/.test(file.sha256 ?? '')
        || file.url !== `https://cdn.axiom.stream/downloads/${value.version}/${file.name}`
        || file.githubUrl !== `${GITHUB_RELEASES}/download/v${value.version}/${file.name}`) throw new Error('Invalid release artifact');
      seen.add(key);
    }
  }
  return structuredClone(value);
}

export function requireCompleteRelease(value) {
  const release = parseRelease(value);
  const expected = new Set([
    ...['desktop','cli'].flatMap(product => ['mac','win'].flatMap(platform => (release.schemaVersion === 3 ? ['universal'] : ['x64','arm64']).map(arch=>`${product}:${platform}:${arch}:${platform === 'mac' ? 'pkg' : 'exe'}`))),
    ...['AppImage','deb','pacman'].map(format=>`desktop:linux:x64:${format}`), 'cli:linux:x64:sh',
  ]);
  for (const file of [...release.downloads,...release.cliDownloads]) {
    if (!expected.delete(`${file.product}:${file.platform}:${file.arch}:${file.format}`)) throw new Error('Unexpected release target');
  }
  if (expected.size) throw new Error(`Incomplete release: ${[...expected].join(', ')}`);
  return release;
}

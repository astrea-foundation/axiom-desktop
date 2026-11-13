// Public release metadata shared by the website, packaging, and native updater.
export const RELEASE_API = 'https://axiom.stream/api/releases/latest';
export const VERSION_PATTERN = /^(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})$/;
export const FORMATS = { mac: ['pkg', 'dmg', 'zip'], win: ['exe'], linux: ['AppImage', 'deb', 'pacman'] };

export function compareVersions(a, b) {
  if (!VERSION_PATTERN.test(a) || !VERSION_PATTERN.test(b)) throw new Error('Invalid release version');
  const left = a.split('.').map(Number), right = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return Math.sign(left[i] - right[i]);
  return 0;
}

export function artifactFormat(name) {
  if (/\.pkg\.tar\.(xz|zst)$/.test(name)) return 'pacman';
  return /\.(AppImage|deb|dmg|pkg|zip|exe)$/.exec(name)?.[1] ?? null;
}

export function parseRelease(value) {
  if (!value || typeof value !== 'object' || typeof value.version !== 'string' || !VERSION_PATTERN.test(value.version)
    || (value.schemaVersion !== undefined && value.schemaVersion !== 1)
    || !['signed', 'unsigned'].includes(value.signing)
    || !Array.isArray(value.downloads) || !value.downloads.length || value.downloads.length > 100) throw new Error('Invalid release manifest');
  const seen = new Set();
  const downloads = value.downloads.map(file => {
    if (!file || typeof file !== 'object' || typeof file.name !== 'string'
      || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,199}$/.test(file.name) || !file.name.includes(`-${value.version}-`)
      || !['x64', 'arm64'].includes(file.arch) || !Object.hasOwn(FORMATS, file.platform)) throw new Error('Invalid release artifact');
    const format = artifactFormat(file.name);
    const namedArch = /-arm64[.-]/.test(file.name) ? 'arm64' : /-(x64|x86_64|amd64)[.-]/.test(file.name) ? 'x64' : null;
    const key = `${file.platform}:${file.arch}:${format}`;
    if (!FORMATS[file.platform].includes(format) || namedArch !== file.arch || (file.format !== undefined && file.format !== format)
      || seen.has(key) || !Number.isSafeInteger(file.bytes) || file.bytes < 1 || file.bytes > 2 * 1024 ** 3
      || typeof file.sha256 !== 'string' || !/^[a-f0-9]{64}$/.test(file.sha256)
      || file.url !== `https://cdn.axiom.stream/downloads/${value.version}/${file.name}`) throw new Error('Invalid release download');
    seen.add(key);
    return { name: file.name, platform: file.platform, arch: file.arch, format, bytes: file.bytes, sha256: file.sha256, url: file.url };
  });
  // Additive schema-1 extension: older desktop updaters continue to consume only
  // downloads. CLI archives must never become desktop installer candidates.
  const cliDownloads = value.cliDownloads === undefined ? [] : value.cliDownloads;
  if (!Array.isArray(cliDownloads) || downloads.length + cliDownloads.length > 100) throw new Error('Invalid CLI downloads');
  const cliSeen = new Set();
  const cli = cliDownloads.map(file => {
    if (!file || typeof file !== 'object' || file.platform !== 'linux' || file.arch !== 'x64'
      || file.format !== 'tar.gz' || file.name !== `axiomai-${value.version}-x86_64-unknown-linux-gnu.tar.gz`
      || cliSeen.has(`${file.platform}:${file.arch}`)
      || !Number.isSafeInteger(file.bytes) || file.bytes < 1 || file.bytes > 2 * 1024 ** 3
      || typeof file.sha256 !== 'string' || !/^[a-f0-9]{64}$/.test(file.sha256)
      || file.url !== `https://cdn.axiom.stream/downloads/${value.version}/${file.name}`) throw new Error('Invalid CLI download');
    cliSeen.add(`${file.platform}:${file.arch}`);
    return { name: file.name, platform: file.platform, arch: file.arch, format: file.format, bytes: file.bytes, sha256: file.sha256, url: file.url };
  });
  // Keep historical manifests byte-compatible after normalization.
  return { schemaVersion: 1, version: value.version, signing: value.signing, downloads, ...(cli.length ? { cliDownloads: cli } : {}) };
}

import {mkdir, readFile, readdir, writeFile} from 'node:fs/promises';
import {compareVersions, parseRelease} from '../../packages/desktop-releases/manifest.mjs';

export async function buildDownloads() {
  const release = parseRelease(JSON.parse(await readFile('releases.json', 'utf8')));
  const versions = new Map([[release.version, release]]);
  for (const name of await readdir('releases')) {
    if (!name.endsWith('.json')) continue;
    const archived = parseRelease(JSON.parse(await readFile(`releases/${name}`, 'utf8')));
    if (name !== `${archived.version}.json` || compareVersions(archived.version, release.version) > 0) throw new Error('Invalid release archive');
    if (versions.has(archived.version) && JSON.stringify(versions.get(archived.version)) !== JSON.stringify(archived)) throw new Error('Conflicting release archive');
    versions.set(archived.version, archived);
  }
  const releases = [...versions.values()].sort((a, b) => compareVersions(b.version, a.version));
  await mkdir('.site/releases', {recursive: true});
  const json = value => JSON.stringify(value, null, 2) + '\n';
  await writeFile('.site/releases/latest.json', json(release));
  await writeFile('.site/releases/index.json', json({schemaVersion: 1, latest: release.version, releases}));
  for (const item of releases) await writeFile(`.site/releases/${item.version}.json`, json(item));
  const link = (platform, arch, extension, label) => {
    const file = release.downloads.find(f => f.platform === platform && f.arch === arch && f.name.endsWith(extension));
    if (!file) throw new Error(`Missing ${platform} ${arch} ${extension}`);
    return `<a class="download-link" href="${file.url}"><span>${label}</span><small>${Math.round(file.bytes / 1024 / 1024)} MB ↗</small></a>`;
  };
  const html = await readFile('downloads.template.html', 'utf8');
  const replacements = {
    VERSION: release.version,
    HISTORY: releases.map(item => `<details><summary>Version ${item.version}${item.version === release.version ? ' · Latest' : ''}</summary><div class="download-list">${item.downloads.map(f => `<a class="download-link" href="${f.url}"><span>${{mac:'macOS',win:'Windows',linux:'Linux'}[f.platform]} · ${f.arch} · ${f.format}</span><small>${Math.round(f.bytes / 1024 / 1024)} MB ↗</small></a>`).join('')}</div><a class="text-link" href="/api/releases/${item.version}">Manifest and checksums ↗</a></details>`).join(''),
    MAC: link('mac','arm64','.pkg','Apple Silicon · PKG') + link('mac','x64','.pkg','Intel · PKG'),
    MAC_OTHER: link('mac','arm64','.dmg','Apple Silicon · DMG') + link('mac','x64','.dmg','Intel · DMG') + link('mac','arm64','.zip','Apple Silicon · ZIP') + link('mac','x64','.zip','Intel · ZIP'),
    WINDOWS: link('win','x64','.exe','Windows x64 · EXE') + link('win','arm64','.exe','Windows ARM64 · EXE'),
    LINUX: link('linux','x64','.AppImage','AppImage') + link('linux','x64','.deb','Debian / Ubuntu · DEB') + link('linux','x64','.pkg.tar.xz','Arch Linux · Package'),
    CHECKSUMS: release.downloads.map(f => `<li><span>${f.name}</span><code>${f.sha256}</code></li>`).join(''),
  };
  await writeFile('.site/downloads.html', html.replace(/\{\{([A-Z_]+)\}\}/g, (_, key) => {
    if (!(key in replacements)) throw new Error(`Unknown download token ${key}`);
    return replacements[key];
  }));
}

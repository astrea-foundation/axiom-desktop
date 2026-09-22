import { FORMATS, parseRelease } from '../packages/desktop-releases/manifest.mjs';

/** Synthetic update metadata. Browser tests block every external request. */
export function desktopReleaseFixture(version = '0.2.0') {
  return parseRelease({ schemaVersion: 2, version, revision: 'a'.repeat(40), sequence: 100, signing: 'signed', signature: {keyId: 'a'.repeat(16), value: 'A'.repeat(86)+'=='}, cliDownloads: [], downloads: Object.entries(FORMATS).flatMap(([platform, formats]) =>
    ['x64', 'arm64'].flatMap(arch => formats.map(format => {
      const name = `Axiom-${version}-${platform}-${arch}.${format === 'pacman' ? 'pkg.tar.xz' : format}`;
      return { name, product: 'desktop', platform, arch, format, bytes: 50 * 1024 * 1024, sha256: 'a'.repeat(64),
        url: `https://cdn.axiom.stream/downloads/${version}/${name}`, githubUrl: `https://github.com/astrea-foundation/axiom-releases/releases/download/v${version}/${name}` };
    }))) });
}

import { FORMATS, parseRelease } from '../packages/desktop-releases/manifest.mjs';

/** Synthetic update metadata. Browser tests block every external request. */
export function desktopReleaseFixture(version = '0.2.0') {
  return parseRelease({ version, signing: 'unsigned', downloads: Object.entries(FORMATS).flatMap(([platform, formats]) =>
    ['x64', 'arm64'].flatMap(arch => formats.map(format => {
      const name = `Axiom-${version}-${platform}-${arch}.${format === 'pacman' ? 'pkg.tar.xz' : format}`;
      return { name, platform, arch, format, bytes: 50 * 1024 * 1024, sha256: 'a'.repeat(64),
        url: `https://cdn.axiom.stream/downloads/${version}/${name}` };
    }))) });
}

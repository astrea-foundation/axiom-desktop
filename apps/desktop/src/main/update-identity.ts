import { FORMATS, type PackageFormat } from '../../../../packages/desktop-releases/manifest.mjs';
import type { UpdateIdentity } from '../shared/updates';

export function updateIdentity(input: {
  version: string; platform: string; arch: string; packaged: boolean; packageFormat?: unknown; appImage?: string;
}): UpdateIdentity {
  const platform = input.platform === 'darwin' ? 'mac' : input.platform === 'win32' ? 'win' : input.platform === 'linux' ? 'linux' : null;
  const arch = input.arch === 'x64' || input.arch === 'arm64' ? input.arch : null;
  const format = platform && typeof input.packageFormat === 'string' && FORMATS[platform].includes(input.packageFormat as PackageFormat)
    ? input.packageFormat as PackageFormat : null;
  return { currentVersion: input.version, platform, arch, format, packaged: input.packaged };
}

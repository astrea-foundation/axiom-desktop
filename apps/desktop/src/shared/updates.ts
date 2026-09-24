import type { PackageFormat, Release, ReleaseArch, ReleaseDownload, ReleasePlatform } from '../../../../packages/desktop-releases/manifest.mjs';
export type { PackageFormat, ReleaseDownload } from '../../../../packages/desktop-releases/manifest.mjs';

export type UpdateIdentity = {
  currentVersion: string;
  platform: ReleasePlatform | null;
  arch: ReleaseArch | null;
  format: PackageFormat | null;
  packaged: boolean;
};

/** Keep Desktop selection consistent with the native updater's target matching. */
export function matchesUpdateTarget(file: ReleaseDownload, identity: UpdateIdentity): boolean {
  return file.product === 'desktop' && file.platform === identity.platform && file.format === identity.format
    && (identity.arch === 'x64' || identity.arch === 'arm64')
    && (file.arch === identity.arch || (file.arch === 'universal' && (identity.platform === 'mac' || identity.platform === 'win')));
}
export type UpdateState = UpdateIdentity & {
  revision: number;
  status: 'disabled' | 'idle' | 'checking' | 'current' | 'available' | 'downloading' | 'waiting' | 'installing' | 'error';
  release: Release | null;
  checkedAt: number | null;
  error: string | null;
  download: { name: string; received: number; total: number } | null;
};
export type UpdatesApi = {
  getState: () => Promise<UpdateState>;
  onBeforeRestart: (callback: () => Promise<void>) => () => void;
  check: () => Promise<UpdateState>;
  install: () => Promise<UpdateState>;
  cancel: () => Promise<UpdateState>;
  onStateChange: (callback: (state: UpdateState) => void) => () => void;
};

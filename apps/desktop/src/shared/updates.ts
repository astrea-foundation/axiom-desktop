import type { PackageFormat, Release, ReleaseArch, ReleasePlatform } from '../../../../packages/desktop-releases/manifest.mjs';
export type { PackageFormat, ReleaseDownload } from '../../../../packages/desktop-releases/manifest.mjs';

export type UpdateIdentity = {
  currentVersion: string;
  platform: ReleasePlatform | null;
  arch: ReleaseArch | null;
  format: PackageFormat | null;
  packaged: boolean;
};
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

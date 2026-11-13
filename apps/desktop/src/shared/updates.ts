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
  status: 'disabled' | 'idle' | 'checking' | 'current' | 'available' | 'downloading' | 'downloaded' | 'error';
  release: Release | null;
  checkedAt: number | null;
  error: string | null;
  download: { name: string; received: number; total: number } | null;
};
export type UpdatesApi = {
  getState: () => Promise<UpdateState>;
  check: () => Promise<UpdateState>;
  download: (format?: PackageFormat) => Promise<UpdateState>;
  reveal: () => Promise<void>;
  onStateChange: (callback: (state: UpdateState) => void) => () => void;
};

import { compareVersions, parseRelease, RELEASE_API, VERSION_PATTERN } from '../../../../packages/desktop-releases/manifest.mjs';
import type { PackageFormat, ReleaseDownload, UpdateIdentity, UpdateState } from '../shared/updates';
import { readUpdateJson, saveUpdate } from './update-download';

type Dependencies = {
  fetch?: typeof fetch;
  chooseDestination: (file: ReleaseDownload) => Promise<string | null>;
  reveal: (path: string) => void;
  emit: (state: UpdateState) => void;
  save?: typeof saveUpdate;
};

export class UpdateService {
  private state: UpdateState;
  private checking: Promise<UpdateState> | null = null;
  private downloading: Promise<UpdateState> | null = null;
  private controller = new AbortController();
  private timer?: ReturnType<typeof setInterval>;
  private downloadedPath: string | null = null;

  constructor(identity: UpdateIdentity, private dependencies: Dependencies) {
    this.state = { ...identity, revision: 0, status: identity.packaged && identity.platform && identity.arch && VERSION_PATTERN.test(identity.currentVersion) ? 'idle' : 'disabled',
      release: null, checkedAt: null, error: null, download: null };
  }
  snapshot(): UpdateState { return structuredClone(this.state); }
  private set(patch: Partial<UpdateState>): UpdateState {
    this.state = { ...this.state, ...patch, revision: this.state.revision + 1 };
    this.dependencies.emit(this.snapshot());
    return this.snapshot();
  }
  start(): void {
    if (this.timer || this.state.status === 'disabled') return;
    void this.check();
    this.timer = setInterval(() => { void this.check(); }, 6 * 60 * 60 * 1000);
    this.timer.unref();
  }
  async dispose(): Promise<void> {
    clearInterval(this.timer);
    this.controller.abort();
    await Promise.allSettled([this.checking, this.downloading]);
  }
  check(): Promise<UpdateState> {
    if (this.checking) return this.checking;
    if (this.downloading || this.state.status === 'disabled' || this.controller.signal.aborted) return Promise.resolve(this.snapshot());
    this.checking = this.performCheck().finally(() => { this.checking = null; });
    return this.checking;
  }
  private async performCheck(): Promise<UpdateState> {
    const previous = this.snapshot();
    this.set({ status: 'checking', error: null, download: null });
    try {
      const response = await (this.dependencies.fetch ?? fetch)(RELEASE_API, {
        redirect: 'error', credentials: 'omit', cache: 'no-store',
        signal: AbortSignal.any([this.controller.signal, AbortSignal.timeout(15_000)]),
        headers: { Accept: 'application/json' },
      });
      const release = parseRelease(await readUpdateJson(response));
      if (previous.status === 'downloaded' && previous.release?.version === release.version) {
        const accepted = previous.release.downloads.find(file => file.name === previous.download?.name);
        const advertised = release.downloads.find(file => file.name === previous.download?.name);
        if (!accepted || !advertised || JSON.stringify(accepted) !== JSON.stringify(advertised)) throw new Error('Published download changed');
        return this.set({ status: 'downloaded', release, download: previous.download, checkedAt: Date.now() });
      }
      this.downloadedPath = null;
      return this.set({ status: compareVersions(release.version, this.state.currentVersion) > 0 ? 'available' : 'current', release, checkedAt: Date.now() });
    } catch {
      return this.set({ status: previous.status === 'downloaded' ? 'downloaded' : 'error',
        release: previous.status === 'downloaded' ? previous.release : null,
        download: previous.status === 'downloaded' ? previous.download : null,
        error: 'Couldn’t check for updates. Try again when you’re online.' });
    }
  }
  download(format?: PackageFormat): Promise<UpdateState> {
    if (this.downloading) return this.downloading;
    if (this.checking || !this.state.release || !['available', 'error'].includes(this.state.status)
      || compareVersions(this.state.release.version, this.state.currentVersion) <= 0 || this.controller.signal.aborted) return Promise.resolve(this.snapshot());
    const selectedFormat = this.state.format ?? format;
    const file = this.state.release.downloads.find(item => item.platform === this.state.platform && item.arch === this.state.arch && item.format === selectedFormat);
    if (!file || (this.state.format && format && format !== this.state.format)) return Promise.reject(new Error('Choose a matching installer format'));
    this.downloading = this.performDownload(file).finally(() => { this.downloading = null; });
    return this.downloading;
  }
  private async performDownload(file: ReleaseDownload): Promise<UpdateState> {
    try {
      this.set({ status: 'downloading', error: null, download: { name: file.name, received: 0, total: file.bytes } });
      const destination = await this.dependencies.chooseDestination(file);
      if (!destination) return this.set({ status: 'available', download: null });
      this.controller.signal.throwIfAborted();
      let lastProgress = 0;
      await (this.dependencies.save ?? saveUpdate)(file, destination,
        AbortSignal.any([this.controller.signal, AbortSignal.timeout(30 * 60 * 1000)]), received => {
          if (Date.now() - lastProgress < 200 && received !== file.bytes) return;
          lastProgress = Date.now();
          this.set({ download: { name: file.name, received, total: file.bytes } });
        }, this.dependencies.fetch ?? fetch);
      this.downloadedPath = destination;
      return this.set({ status: 'downloaded', download: { name: file.name, received: file.bytes, total: file.bytes } });
    } catch {
      return this.set({ status: 'error', download: null, error: 'Couldn’t download and verify the update. Check your connection and free space, then try again.' });
    }
  }
  reveal(): void {
    if (this.state.status === 'downloaded' && this.downloadedPath) this.dependencies.reveal(this.downloadedPath);
  }
}

import { compareVersions, parseRelease, VERSION_PATTERN } from '../../../../packages/desktop-releases/manifest.mjs';
import { matchesUpdateTarget, type UpdateIdentity, type UpdateState } from '../shared/updates';

export type NativeUpdateEvent = {event: string; lastError?: string | null; release?: unknown; installation?: {product: string; format: string}; name?: string; received?: number; total?: number; job?: string; message?: string};
type Dependencies = {
  run: (args: string[], receive: (event: NativeUpdateEvent) => void, signal: AbortSignal) => Promise<void>;
  canRestart: () => boolean;
  prepareRestart: () => Promise<void>;
  restart: () => Promise<void>;
  emit: (state: UpdateState) => void;
  parentPid: number;
};

export class UpdateService {
  private state: UpdateState;
  private operation: Promise<UpdateState> | null = null;
  private controller = new AbortController();
  private timer?: ReturnType<typeof setInterval>;
  private disposed = false;
  constructor(identity: UpdateIdentity, private dependencies: Dependencies) {
    this.state = {...identity, revision: 0, status: identity.packaged && identity.platform && identity.arch && identity.format && VERSION_PATTERN.test(identity.currentVersion) ? 'idle' : 'disabled', release: null, checkedAt: null, error: null, download: null};
  }
  snapshot(): UpdateState { return structuredClone(this.state); }
  private set(patch: Partial<UpdateState>): UpdateState {
    this.state = {...this.state, ...patch, revision: this.state.revision + 1};
    this.dependencies.emit(this.snapshot()); return this.snapshot();
  }
  start(): void {
    if (this.timer || this.state.status === 'disabled') return;
    void this.check(); this.timer = setInterval(() => { void this.check(); }, 6 * 60 * 60 * 1000); this.timer.unref();
  }
  async dispose(): Promise<void> { this.disposed = true; clearInterval(this.timer); this.controller.abort(); await this.operation; }
  cancel(): UpdateState { if (this.state.status !== 'installing') this.controller.abort(); return this.snapshot(); }
  check(): Promise<UpdateState> { return this.perform(false); }
  install(): Promise<UpdateState> { return this.perform(true); }
  private perform(install: boolean): Promise<UpdateState> {
    if (this.operation) return this.operation;
    if (this.disposed || this.state.status === 'disabled') return Promise.resolve(this.snapshot());
    this.controller = new AbortController();
    this.operation = this.execute(install).finally(() => { this.operation = null; });
    return this.operation;
  }
  private async execute(install: boolean): Promise<UpdateState> {
    const signal = this.controller.signal;
    this.set({status: 'checking', error: null, download: null});
    let checked = false, job: string | null = null;
    try {
      await this.dependencies.run(install ? ['--prepare', '--desktop'] : ['--check'], event => {
        if (event.event === 'checked') {
          const release = parseRelease(event.release);
          if (release.signing !== 'signed' || event.installation?.product !== 'desktop' || event.installation.format !== this.state.format) throw new Error('Update installation identity mismatch');
          checked = true;
          this.set({release, checkedAt: Date.now(), error: event.lastError ?? null, status: compareVersions(release.version, this.state.currentVersion) > 0 ? 'available' : 'current'});
        } else if (event.event === 'progress') {
          const file = this.state.release?.downloads.find(file => file.name === event.name && matchesUpdateTarget(file, this.state));
          if (!checked || !file || event.total !== file.bytes || !Number.isSafeInteger(event.received) || event.received! < 0 || event.received! > file.bytes) throw new Error('Invalid update progress');
          this.set({status: 'downloading', download: {name: file.name, received: event.received!, total: file.bytes}});
        } else if (event.event === 'ready') {
          if (!checked || typeof event.job !== 'string' || !event.job) throw new Error('Invalid update handoff');
          job = event.job;
        } else throw new Error('Unexpected native update event');
      }, signal);
      if (!checked) throw new Error('Update check ended without an authenticated release');
      if (!install || this.state.status === 'current') return this.snapshot();
      if (!job) throw new Error('Update download ended before verification');
      this.set({status: 'waiting'});
      while (!this.dependencies.canRestart()) {
        await new Promise<void>((resolve, reject) => {
          const abort = () => { clearTimeout(timer); reject(signal.reason); };
          const timer = setTimeout(() => { signal.removeEventListener('abort', abort); resolve(); }, 500);
          signal.addEventListener('abort', abort, {once: true});
          if (signal.aborted) abort();
        });
      }
      signal.throwIfAborted();
      this.set({status: 'installing'});
      await this.dependencies.prepareRestart();
      signal.throwIfAborted();
      let started = false;
      await this.dependencies.run(['--start-job', job, '--parent', String(this.dependencies.parentPid)], event => {
        if (event.event !== 'installing') throw new Error('Invalid installer acknowledgement');
        started = true;
      }, signal);
      if (!started) throw new Error('Installer did not acknowledge the update');
      this.set({status: 'installing'});
      await this.dependencies.restart();
      return this.snapshot();
    } catch (error) {
      if (signal.aborted) return this.set({status: this.state.release ? 'available' : 'idle', download: null, error: null});
      return this.set({status: 'error', download: null, error: error instanceof Error ? error.message : 'Could not update Axiom. Your installation has not been replaced.'});
    }
  }
}

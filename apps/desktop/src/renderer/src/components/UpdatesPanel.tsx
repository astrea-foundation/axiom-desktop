import { ArrowDownToLine, ArrowUpRight, Check, LoaderCircle, RefreshCw, X } from 'lucide-react';
import type { DesktopUpdates } from '../useDesktopUpdates';
import { matchesUpdateTarget } from '../../../shared/updates';

const mb = (bytes: number) => `${Math.round(bytes / 1024 / 1024)} MB`;
const button = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-2 text-[12.5px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--surface-button-secondary-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:opacity-45';

export function UpdatesPanel({ updates }: { updates?: DesktopUpdates }) {
  const state = updates?.state;
  if (!state) return <p role="status" className="text-[13px] text-[var(--color-text-secondary)]">{updates?.error ?? 'Update controls are available in the desktop app.'}</p>;
  const {status, release} = state;
  const busy = ['checking', 'downloading', 'waiting', 'installing'].includes(status);
  const available = release && ['available', 'downloading', 'waiting', 'installing', 'error'].includes(status);
  const file = release?.downloads.find(file => matchesUpdateTarget(file, state));
  const heading = status === 'installing' ? 'Installing update…'
    : status === 'waiting' ? 'Ready to restart'
    : status === 'downloading' ? `Downloading Axiom ${release?.version}`
    : status === 'checking' ? 'Checking for updates…'
    : status === 'error' ? 'Update could not complete'
    : available ? `Axiom ${release.version} is available`
    : status === 'current' ? 'You’re up to date'
    : status === 'disabled' ? state.packaged ? 'Updates unavailable for this build' : 'Development build'
    : 'Keep Axiom up to date';
  const description = status === 'waiting' ? 'Finish active chats and stop the local proxy. Axiom will then restart. Other AxiomCLI sessions must also be closed.'
    : status === 'installing' ? 'Axiom will reopen when installation finishes. Your chats and settings are kept.'
    : status === 'disabled' ? 'Automatic updates are available in installed releases.'
    : available && !file ? 'A matching installer has not been published yet.'
    : 'Update downloads and verifies the new release, installs it, and restarts Axiom.';
  const progress = state.download ? Math.floor(state.download.received / state.download.total * 100) : 0;
  const Icon = status === 'current' ? Check : busy ? LoaderCircle : ArrowDownToLine;
  return <div className="max-w-[560px]">
    <p className="mb-4 text-[12px] text-[var(--color-text-tertiary)]">Installed version: {state.currentVersion}</p>
    <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
      <div className="flex items-start gap-3">
        <Icon size={18} className={busy ? 'animate-spin motion-reduce:animate-none' : ''} aria-hidden="true" />
        <div><h3 role="status" className="text-[14px] font-medium text-[var(--color-text-primary)]">{heading}</h3>
          <p className="mt-1 text-[12px] leading-5 text-[var(--color-text-tertiary)]">{description}</p></div>
      </div>
      {status === 'downloading' ? <div className="mt-5">
        <div role="progressbar" aria-label="Update download" aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100} className="h-1.5 overflow-hidden rounded-full bg-[var(--surface-track)]">
          <div className="h-full rounded-full bg-[var(--color-text-secondary)]" style={{width: `${progress}%`}} />
        </div>
        <p className="mt-2 text-[11.5px] tabular-nums text-[var(--color-text-tertiary)]">{progress}% · {mb(state.download?.received ?? 0)} of {mb(state.download?.total ?? 0)}</p>
      </div> : null}
      {updates?.error ?? state.error ? <p role="alert" className="mt-4 text-[12px] leading-5 text-[var(--color-danger-strong)]">{updates?.error ?? state.error}</p> : null}
      <div className="mt-5 flex flex-wrap gap-2">
        {available && !busy ? <button type="button" className={`${button} !bg-[var(--color-cherry)] !text-[var(--color-on-accent)]`} disabled={!file}
          onClick={() => void updates?.install()}><RefreshCw size={14} />Update and restart</button> : null}
        {['checking', 'downloading', 'waiting'].includes(status) ? <button type="button" className={button} onClick={() => void updates?.cancel()}><X size={14} />Cancel</button> : null}
        <button type="button" className={button} disabled={busy || status === 'disabled'} onClick={() => void updates?.check()}><RefreshCw size={13} />Check for updates</button>
      </div>
    </div>
    <div className="mt-4 flex flex-wrap justify-between gap-2 text-[11.5px] text-[var(--color-text-tertiary)]">
      <span>{state.checkedAt ? `Last checked ${new Date(state.checkedAt).toLocaleString()}` : 'No update check yet'}</span>
      <a href="https://github.com/astrea-foundation/axiom-releases/releases" target="_blank" rel="noreferrer" className="inline-flex items-center gap-1">Release downloads<ArrowUpRight size={12} /></a>
    </div>
    {file ? <details className="mt-4 border-t border-[var(--color-border)] pt-4 text-[12px] text-[var(--color-text-tertiary)]">
      <summary className="cursor-pointer">Update details</summary>
      <p className="mt-3 break-all">{file.name} · {mb(file.bytes)}</p>
      <p className="mt-2">The native updater verifies the release signature and downloaded file before installing.</p>
      <code className="mt-2 block select-all break-all text-[11px]">{file.sha256}</code>
    </details> : null}
  </div>;
}

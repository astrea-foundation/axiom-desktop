import { ArrowDownToLine, ArrowUpRight, Check, FolderOpen, LoaderCircle, RefreshCw } from 'lucide-react';
import { useEffect, useId, useState } from 'react';
import type { DesktopUpdates } from '../useDesktopUpdates';
import type { PackageFormat } from '../../../shared/updates';

const formats: Record<PackageFormat, string> = { AppImage: 'AppImage', deb: 'DEB', pacman: 'Arch package', dmg: 'DMG', pkg: 'PKG', zip: 'ZIP', exe: 'EXE' };
const mb = (bytes: number) => `${Math.round(bytes / 1024 / 1024)} MB`;
const button = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-2 text-[12.5px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--surface-button-secondary-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:opacity-45';

export function UpdatesPanel({ updates }: { updates?: DesktopUpdates }) {
  const state = updates?.state;
  const [choice, setChoice] = useState<PackageFormat | ''>('');
  const id = useId();
  useEffect(() => { setChoice(''); }, [state?.release?.version]);
  if (!state) return <p role="status" className="text-[13px] text-[var(--color-text-secondary)]">{updates?.error ?? 'Update controls are available in the desktop app.'}</p>;
  const { status, release } = state;
  const busy = status === 'checking' || status === 'downloading';
  const available = release && ['available', 'downloading', 'downloaded', 'error'].includes(status);
  const options = release?.downloads.filter(file => file.platform === state.platform && file.arch === state.arch && (!state.format || file.format === state.format)) ?? [];
  const file = options.find(item => state.download ? item.name === state.download.name : item.format === (state.format ?? choice));
  const downloading = status === 'downloading', ready = status === 'downloaded';
  const heading = ready ? 'Your update is ready'
    : downloading ? `Downloading Axiom ${release?.version}`
      : status === 'checking' ? 'Checking for updates…'
        : available ? `Axiom ${release.version} is available`
          : status === 'current' ? 'You’re up to date'
            : status === 'disabled' ? state.packaged ? 'Updates unavailable for this build' : 'Development build'
              : status === 'error' ? 'Updates unavailable' : 'Keep Axiom up to date';
  const Icon = ready || status === 'current' ? Check : busy ? LoaderCircle : ArrowDownToLine;
  const platform = state.platform ? {mac: 'macOS', win: 'Windows', linux: 'Linux'}[state.platform] : 'Unsupported platform';
  const progress = state.download ? Math.floor(state.download.received / state.download.total * 100) : 0;
  const error = updates?.error ?? state.error;
  return <div className="max-w-[560px]">
    <p className="mb-4 text-[12px] text-[var(--color-text-tertiary)]">Installed · Axiom {state.currentVersion} · {platform} · {state.arch ?? 'Unknown architecture'}{state.format ? ` · ${formats[state.format]}` : ''}</p>
    <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
      <div className="flex items-start gap-3">
        <span className="shadow-glass-tile flex size-9 shrink-0 items-center justify-center rounded-[10px] bg-[var(--surface-tile)] text-[var(--color-text-secondary)]">
          <Icon size={18} className={busy ? 'animate-spin motion-reduce:animate-none' : ''} aria-hidden="true" />
        </span>
        <div className="min-w-0 pt-0.5">
          <h3 role="status" className="text-[14px] font-medium text-[var(--color-text-primary)]">{heading}</h3>
          <p className="mt-1 text-[12px] leading-5 text-[var(--color-text-tertiary)]">
            {ready ? 'Quit Axiom and all AxiomCLI sessions, then install the update to replace Desktop and its bundled CLI together.'
              : downloading ? 'You can keep chatting while it downloads.'
                : status === 'disabled' ? 'Installed releases check for updates automatically.'
                  : available && file ? `${platform} · ${state.arch} · ${formats[file.format]} · ${mb(file.bytes)}`
                    : available && !options.length ? 'A matching build hasn’t been published for this installation yet.'
                      : available ? 'Choose the installer format used for this older installation.'
                        : 'Axiom checks automatically every six hours.'}
          </p>
        </div>
      </div>
      {available && !state.format && options.length > 0 && !downloading && !ready ? <div className="mt-4">
        <label htmlFor={id} className="mb-1.5 block text-[12px] text-[var(--color-text-secondary)]">Installer format</label>
        <select id={id} value={choice} onChange={event => setChoice(event.target.value as PackageFormat)}
          className="w-full rounded-lg border border-[var(--color-border)] bg-[var(--surface-tile)] px-3 py-2 text-[13px] text-[var(--color-text-primary)]">
          <option value="" disabled>Choose a format</option>
          {options.map(item => <option key={item.format} value={item.format}>{formats[item.format]} · {mb(item.bytes)}</option>)}
        </select>
      </div> : null}
      {downloading ? <div className="mt-5">
        <div role="progressbar" aria-label="Update download" aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100}
          className="h-1.5 overflow-hidden rounded-full bg-[var(--surface-track)]">
          <div className="h-full rounded-full bg-[var(--color-text-secondary)] transition-[width] motion-reduce:transition-none" style={{width: `${progress}%`}} />
        </div>
        <p className="mt-2 text-[11.5px] tabular-nums text-[var(--color-text-tertiary)]">{progress === 100 ? 'Verifying download…' : `${progress}% · ${mb(state.download?.received ?? 0)} of ${mb(state.download?.total ?? 0)}`}</p>
      </div> : null}
      {error ? <p role="alert" className="mt-4 text-[12px] leading-5 text-[var(--color-danger-strong)]">{error}</p> : null}
      <div className="mt-5 flex flex-wrap gap-2">
        {ready ? <button type="button" className={button} onClick={() => void updates?.reveal()}><FolderOpen size={14} />Show in folder</button>
          : available && !downloading ? <button type="button" className={`${button} !bg-[var(--color-cherry)] !text-[var(--color-on-accent)] hover:!bg-[var(--color-cherry-bright)]`} disabled={!file || busy}
            onClick={() => void updates?.download(file?.format)}><ArrowDownToLine size={14} />Download update</button> : null}
        <button type="button" className={button} disabled={busy || status === 'disabled'} onClick={() => void updates?.check()}>
          <RefreshCw size={13} className={status === 'checking' ? 'animate-spin motion-reduce:animate-none' : ''} />{status === 'checking' ? 'Checking…' : 'Check for updates'}
        </button>
      </div>
    </div>
    <div className="mt-4 flex flex-wrap items-center justify-between gap-2 text-[11.5px] text-[var(--color-text-tertiary)]">
      <span>{state.checkedAt ? `Last checked ${new Date(state.checkedAt).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}` : 'No update check yet'}</span>
      <a href="https://axiom.stream/api/releases" target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 hover:text-[var(--color-text-primary)]">Release catalog<ArrowUpRight size={12} /></a>
    </div>
    {(file || ready) && release ? <details className="mt-6 border-t border-[var(--color-border)] pt-4 text-[12px] text-[var(--color-text-tertiary)]">
      <summary className="cursor-pointer select-none hover:text-[var(--color-text-primary)]">Download details</summary>
      <div className="mt-3 space-y-3 leading-5">
        <p className="break-all text-[var(--color-text-secondary)]">{state.download?.name ?? file?.name}</p>
        <p>{ready ? 'File size and SHA-256 matched the release manifest.' : 'The download is checked against the release’s file size and SHA-256.'}</p>
        {file ? <div><span>SHA-256</span><code className="mt-1 block select-all break-all text-[11px]">{file.sha256}</code></div> : null}
        <a href={`https://axiom.stream/api/releases/${release.version}`} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 hover:text-[var(--color-text-primary)]">Release manifest<ArrowUpRight size={12} /></a>
      </div>
    </details> : null}
  </div>;
}

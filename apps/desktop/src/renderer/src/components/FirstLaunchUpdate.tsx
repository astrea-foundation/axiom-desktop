import {ArrowDownToLine, LoaderCircle} from 'lucide-react';
import type {DesktopUpdates} from '../useDesktopUpdates';

export function FirstLaunchUpdate({updates}: {updates: DesktopUpdates}) {
  const state = updates.state!;
  const applying = state.status === 'installing';
  const failed = state.status === 'error' || !!updates.error;
  const retry = failed || state.status === 'available';
  const progress = state.download ? Math.floor(state.download.received / state.download.total * 100) : null;
  const heading = failed ? 'Couldn’t update Axiom' : applying ? 'Restarting into the latest Axiom…' : 'Getting the latest Axiom…';
  const detail = applying ? 'Axiom will reopen when installation finishes.'
    : state.status === 'waiting' ? 'Waiting for other Axiom activity to finish before restarting.'
    : progress !== null ? `Downloading version ${state.release?.version} · ${progress}%`
    : retry ? 'You can retry or continue with the installed version.' : 'Checking for a newer version before you get started.';
  return <section aria-label="First launch update" className="flex h-full min-h-0 flex-1 items-center justify-center p-6">
    <div className="w-full max-w-[430px] rounded-2xl border border-[var(--color-border)] bg-[var(--wash-row)] p-7">
      {retry ? <ArrowDownToLine size={24} aria-hidden="true" /> : <LoaderCircle size={24} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />}
      <h1 role="status" className="mt-5 text-lg font-medium">{heading}</h1>
      <p className="mt-2 text-[13px] leading-6 text-[var(--color-text-secondary)]">{detail}</p>
      {progress !== null ? <div role="progressbar" aria-label="Update download" aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100} className="mt-5 h-1.5 overflow-hidden rounded-full bg-[var(--surface-track)]">
        <div className="h-full rounded-full bg-[var(--color-text-secondary)] transition-[width] motion-reduce:transition-none" style={{width: `${progress}%`}} />
      </div> : null}
      {failed ? <p role="alert" className="mt-4 text-[12px] text-[var(--color-danger-strong)]">{updates.error ?? state.error}</p> : null}
      <div className="mt-6 flex gap-3">
        {retry ? <button type="button" className="rounded-lg bg-[var(--color-cherry)] px-4 py-2 text-[13px] text-[var(--color-on-accent)]" onClick={() => void updates.install()}>Retry update</button> : null}
        {!applying ? <button type="button" className="rounded-lg border border-[var(--color-border)] px-4 py-2 text-[13px]" onClick={() => void updates.continue?.()}>Use installed version</button> : null}
      </div>
      <p className="mt-5 text-[11px] text-[var(--color-text-tertiary)]">Installed version {state.currentVersion}. No sign-in needed to update.</p>
    </div>
  </section>;
}

import { ArrowDownToLine, Check, LoaderCircle } from 'lucide-react';
import type { UpdateState } from '../../../shared/updates';

export function UpdateNotice({ state, onOpen }: { state: UpdateState | null; onOpen: (trigger: HTMLButtonElement) => void }) {
  if (!state?.release || !['available', 'downloading', 'downloaded'].includes(state.status)) return null;
  const downloading = state.status === 'downloading', ready = state.status === 'downloaded';
  const Icon = ready ? Check : downloading ? LoaderCircle : ArrowDownToLine;
  const label = ready ? 'Update downloaded' : downloading ? 'Downloading update…' : `Axiom ${state.release.version} is available`;
  return <button type="button" onClick={event => onOpen(event.currentTarget)} title={label} aria-label={`${label}. View updates`}
    className="no-drag ml-3 inline-flex shrink-0 items-center gap-1.5 rounded-full bg-[var(--wash-chip)] px-2.5 py-1 text-[11.5px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]">
    <Icon size={13} className={downloading ? 'animate-spin motion-reduce:animate-none' : ''} aria-hidden="true" />
    <span className="hidden sm:inline">{label}</span>
  </button>;
}

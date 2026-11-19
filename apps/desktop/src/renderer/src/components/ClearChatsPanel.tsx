import { Trash2 } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState } from "react";

export interface ClearChatsOptions {
  /** Chats currently listed for the signed-in account. */
  count: number;
  /** Why clearing is unavailable, such as when signed out or disconnected. */
  unavailableReason?: string;
  /** Deletes every listed chat and resolves with the number deleted. */
  onClear: () => Promise<number>;
}

const chatsLabel = (count: number) => `${count.toLocaleString()} ${count === 1 ? "chat" : "chats"}`;

export function ClearChatsPanel({ count, unavailableReason, onClear }: ClearChatsOptions) {
  const dialog = useRef<HTMLDialogElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const submitted = useRef(false);
  const [open, setOpen] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const id = useId();
  const disabled = !!unavailableReason || count === 0;

  useLayoutEffect(() => {
    if (open) dialog.current?.showModal();
    else dialog.current?.close();
  }, [open]);

  const close = (cleared = false) => {
    if (submitted.current) return;
    dialog.current?.close();
    setOpen(false);
    setError(null);
    // After clearing, the trigger becomes disabled; keep focus in the panel.
    if (cleared) trigger.current?.closest<HTMLElement>("[role=tabpanel]")?.focus();
    else trigger.current?.focus();
  };
  const clear = async () => {
    if (submitted.current) return;
    submitted.current = true;
    setClearing(true);
    setError(null);
    try {
      const deleted = await onClear();
      submitted.current = false;
      setClearing(false);
      setResult(`Deleted ${chatsLabel(deleted)}.`);
      close(true);
    } catch (error) {
      submitted.current = false;
      setClearing(false);
      setError(error instanceof Error ? error.message : "Your chats could not be deleted. Try again.");
    }
  };

  return <div>
    <h3 className="text-[13.5px] font-medium text-[var(--color-text-primary)]">Clear all chats</h3>
    <p className="mt-2 max-w-[540px] text-[12px] leading-5 text-[var(--color-text-tertiary)]">
      Permanently delete every chat for this account on this computer. Your folders, settings, and sign-in are kept.
    </p>
    <div className="mt-4 flex flex-wrap items-center gap-x-3 gap-y-2">
      <button ref={trigger} type="button" disabled={disabled} onClick={() => { setResult(null); setOpen(true); }}
        className="inline-flex items-center gap-2 rounded-lg bg-[var(--color-danger-soft)] px-3 py-2 text-[12.5px] text-[var(--color-danger-strong)] transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-45">
        <Trash2 size={14} aria-hidden="true" />Clear all chats…
      </button>
      <span role="status" className="text-[12px] text-[var(--color-text-tertiary)]">
        {result ?? unavailableReason ?? (count === 0 ? "No chats to clear." : null)}
      </span>
    </div>
    <dialog ref={dialog} role="alertdialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
      style={{ backgroundColor: "var(--color-bg-elevated)" }}
      onCancel={(event) => { event.preventDefault(); close(); }}
      onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } }}
      className="no-drag glass-panel-solid shadow-glass-pop m-auto max-h-[calc(100%_-_40px)] w-[460px] max-w-[calc(100%_-_40px)] overflow-y-auto rounded-[22px] border-0 p-7 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]">
      <h2 id={`${id}-title`} className="text-[20px] font-medium tracking-tight">Delete all chats?</h2>
      <div id={`${id}-description`} className="mt-3 space-y-3 text-[13px] leading-5 text-[var(--color-text-secondary)]">
        <p>{count === 1 ? "Your only chat" : `All ${chatsLabel(count)}`} for this account will be permanently deleted. Any active responses will be stopped.</p>
        <p>Folders are kept, and files in your working directories are not affected.</p>
        <p className="font-medium text-[var(--color-danger-strong)]">This cannot be undone.</p>
      </div>
      {error ? <p role="alert" className="mt-3 text-[12px] leading-5 text-[var(--color-danger-strong)]">{error}</p> : null}
      {clearing ? <p role="status" className="mt-3 text-[12px] text-[var(--color-text-secondary)]">Deleting chats…</p> : null}
      <div className="mt-6 flex justify-end gap-2">
        <button type="button" autoFocus disabled={clearing} onClick={() => close()}
          className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] hover:bg-[var(--wash-chip-hover)] disabled:opacity-45">Cancel</button>
        <button type="button" disabled={clearing || count === 0} onClick={() => void clear()}
          className="rounded-lg bg-[var(--color-danger-soft)] px-4 py-2 text-[13px] text-[var(--color-danger-strong)] hover:bg-[var(--wash-chip-hover)] disabled:opacity-45">
          {clearing ? "Deleting…" : "Delete all chats"}
        </button>
      </div>
    </dialog>
  </div>;
}

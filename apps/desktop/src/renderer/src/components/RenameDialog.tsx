import { useId, useLayoutEffect, useRef, useState } from "react";

/** Keep editing in the renderer; native JavaScript prompts can break Electron focus. */
export function RenameDialog({ kind, title, onCancel, onSave }: {
  kind: "thread" | "folder";
  title: string;
  onCancel: () => void;
  onSave: (name: string) => Promise<unknown>;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const mounted = useRef(false);
  const submitting = useRef(false);
  const [draft, setDraft] = useState(title);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const errorId = useId();
  const name = draft.trim();
  const tooLong = new TextEncoder().encode(name).length > (kind === "folder" ? 120 : 512);

  useLayoutEffect(() => {
    mounted.current = true;
    const dialog = ref.current!;
    dialog.showModal();
    input.current?.select();
    return () => { mounted.current = false; dialog.close(); };
  }, []);

  const finish = () => {
    ref.current?.close();
    onCancel();
    document.querySelector<HTMLTextAreaElement>("[data-chat-input]")?.focus({ preventScroll: true });
  };

  return (
    <dialog ref={ref} aria-modal="true" aria-labelledby={titleId}
      onCancel={(event) => { event.preventDefault(); if (!submitting.current) finish(); }}
      onKeyDown={(event) => {
        if (event.key !== "Tab") return;
        const controls = ref.current?.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input:not(:disabled), button:not(:disabled)");
        const first = controls?.[0];
        const last = controls?.[controls.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first?.focus();
        }
      }}
      className="no-drag glass-panel-solid shadow-glass-pop m-auto w-[460px] max-w-[calc(100%_-_40px)] rounded-[22px] border-0 p-7 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]">
      <form onSubmit={async (event) => {
        event.preventDefault();
        if (submitting.current || !name || tooLong) return;
        submitting.current = true;
        setSaving(true);
        setError(null);
        try {
          await onSave(name);
          if (mounted.current) finish();
        } catch (cause) {
          if (mounted.current) setError(cause instanceof Error ? cause.message : "Could not rename. Please try again.");
        } finally {
          submitting.current = false;
          if (mounted.current) setSaving(false);
        }
      }}>
        <h2 id={titleId} className="text-[20px] font-medium tracking-tight">Rename {kind}</h2>
        <label className="mt-5 block text-[13px] text-[var(--color-text-secondary)]">
          Name
          <input ref={input} autoFocus value={draft} disabled={saving}
            onChange={(event) => { setDraft(event.target.value); setError(null); }}
            aria-invalid={tooLong || !!error} aria-describedby={tooLong || error ? errorId : undefined}
            className="mt-2 block w-full rounded-lg border border-[var(--color-border-default)] bg-[var(--wash-chip)] px-3 py-2 text-[14px] text-[var(--color-text-primary)] outline-none focus:ring-1 focus:ring-[var(--color-text-tertiary)]" />
        </label>
        {tooLong || error ? <p id={errorId} role="alert" className="mt-3 text-[13px] text-[var(--color-danger-strong)]">{tooLong ? "This name is too long. Please shorten it." : error}</p> : null}
        <div className="mt-6 flex justify-end gap-2">
          <button type="button" disabled={saving} onClick={finish} className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] hover:bg-[var(--wash-chip-hover)] disabled:opacity-50">Cancel</button>
          <button type="submit" disabled={saving || !name || tooLong} className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] hover:bg-[var(--wash-chip-hover)] disabled:opacity-50">{saving ? "Saving…" : "Save"}</button>
        </div>
      </form>
    </dialog>
  );
}

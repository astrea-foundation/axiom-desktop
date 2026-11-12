import { RotateCcw } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState } from "react";

export function FactoryResetPanel() {
  const dialog = useRef<HTMLDialogElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const submitted = useRef(false);
  const [open, setOpen] = useState(false);
  const [confirmation, setConfirmation] = useState("");
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const id = useId();

  useLayoutEffect(() => {
    if (open) dialog.current?.showModal();
    else dialog.current?.close();
  }, [open]);

  const cancel = () => {
    if (submitted.current) return;
    dialog.current?.close();
    setOpen(false);
    setConfirmation("");
    setError(null);
    trigger.current?.focus();
  };
  const reset = async () => {
    if (submitted.current || confirmation !== "RESET") return;
    const desktop = window.axiomDesktop;
    if (!desktop?.factoryReset) return;
    submitted.current = true;
    setResetting(true);
    setError(null);
    try {
      await desktop.factoryReset(confirmation);
    } catch (error) {
      submitted.current = false;
      setResetting(false);
      setError(error instanceof Error ? error.message : "Could not start factory reset. Try again.");
    }
  };

  return <div className="mt-8 border-t border-[var(--color-border)] pt-6">
    <h3 className="text-[13.5px] font-medium text-[var(--color-text-primary)]">Factory reset Axiom</h3>
    <p className="mt-2 max-w-[540px] text-[12px] leading-5 text-[var(--color-text-tertiary)]">
      Start fresh on this computer. Permanently delete all local chats, saved sign-ins, settings, MCP connections, and cached app data, including data shared with AxiomCLI.
    </p>
    <button ref={trigger} type="button" onClick={() => setOpen(true)} disabled={!window.axiomDesktop?.factoryReset}
      className="mt-4 inline-flex items-center gap-2 rounded-lg bg-[var(--color-danger-soft)] px-3 py-2 text-[12.5px] text-[var(--color-danger-strong)] transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-45">
      <RotateCcw size={14} aria-hidden="true" />Factory reset Axiom…
    </button>
    <dialog ref={dialog} role="alertdialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
      style={{ backgroundColor: "var(--color-bg-elevated)" }}
      onCancel={(event) => { event.preventDefault(); cancel(); }}
      onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); cancel(); } }}
      className="no-drag glass-panel-solid shadow-glass-pop m-auto max-h-[calc(100%_-_40px)] w-[500px] max-w-[calc(100%_-_40px)] overflow-y-auto rounded-[22px] border-0 p-7 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]">
      <h2 id={`${id}-title`} className="text-[20px] font-medium tracking-tight">Factory reset Axiom?</h2>
      <div id={`${id}-description`} className="mt-3 space-y-3 text-[13px] leading-5 text-[var(--color-text-secondary)]">
        <p>This permanently deletes Axiom’s data for every account saved on this computer:</p>
        <ul className="list-disc space-y-1 pl-5">
          <li>Chats, attachments, and files in Axiom’s managed chat folders</li>
          <li>Saved sign-ins and MCP credentials in the system credential store</li>
          <li>Desktop and AxiomCLI settings, connections, drafts, and caches</li>
        </ul>
        <p>Your online account, balance, API keys, and files saved outside Axiom are kept.</p>
        <p>Active chats and the local proxy will stop. Close any other AxiomCLI windows before continuing. Axiom will restart, ready for a fresh sign-in.</p>
        <p className="font-medium text-[var(--color-danger-strong)]">This cannot be undone.</p>
      </div>
      <label htmlFor={`${id}-confirmation`} className="mt-5 block text-[12.5px] text-[var(--color-text-secondary)]">Type <strong>RESET</strong> to confirm</label>
      <input id={`${id}-confirmation`} type="text" value={confirmation} onChange={(event) => setConfirmation(event.target.value)}
        disabled={resetting} autoComplete="off" spellCheck={false}
        className="mt-2 w-full rounded-lg border border-[var(--color-border)] bg-[var(--surface-track)] px-3 py-2 text-[13px] outline-offset-2 disabled:opacity-50" />
      {error ? <p role="alert" className="mt-3 text-[12px] leading-5 text-[var(--color-danger-strong)]">{error}</p> : null}
      {resetting ? <p role="status" className="mt-3 text-[12px] text-[var(--color-text-secondary)]">Stopping Axiom and preparing to reset…</p> : null}
      <div className="mt-6 flex justify-end gap-2">
        <button type="button" autoFocus disabled={resetting} onClick={cancel}
          className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] hover:bg-[var(--wash-chip-hover)] disabled:opacity-45">Cancel</button>
        <button type="button" disabled={confirmation !== "RESET" || resetting} onClick={() => void reset()}
          className="rounded-lg bg-[var(--color-danger-soft)] px-4 py-2 text-[13px] text-[var(--color-danger-strong)] hover:bg-[var(--wash-chip-hover)] disabled:opacity-45">
          {resetting ? "Restarting…" : "Delete everything and restart"}
        </button>
      </div>
    </dialog>
  </div>;
}

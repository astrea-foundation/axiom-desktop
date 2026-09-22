import { Bot, FolderOpen, Info, LoaderCircle, ShieldCheck, Terminal, X } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState } from "react";
import type { AgentControlOptions } from "./AgentModeControl";

const PERMISSIONS = [
  { value: "approve_commands", label: "Approve commands", description: "Ask before each tool action.", icon: ShieldCheck },
  { value: "full_access", label: "Full access", description: "Run tools without asking.", icon: Terminal },
] as const;

/** Renderer-owned modal; no OS prompt or composer-positioned overlay. */
export function AgentSettingsDialog({ options, onClose }: { options: AgentControlOptions; onClose(): void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const toggle = useRef<HTMLButtonElement>(null);
  const mounted = useRef(false);
  const inFlight = useRef(false);
  const [draft, setDraft] = useState(options.selection);
  const [busy, setBusy] = useState<"saving" | "choosing" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const id = useId();
  const blocked = !!busy || !!options.disabledReason;

  useLayoutEffect(() => {
    mounted.current = true;
    const element = dialog.current!;
    element.showModal();
    if (!toggle.current?.disabled) toggle.current?.focus({ preventScroll: true });
    return () => { mounted.current = false; element.close(); };
  }, []);

  const finish = () => {
    dialog.current?.close();
    onClose();
  };
  const dismiss = () => { if (!inFlight.current) finish(); };
  const choose = async () => {
    if (inFlight.current || blocked) return;
    inFlight.current = true;
    setBusy("choosing"); setError(null);
    try {
      const selected = await options.onChooseDirectory(draft.workingDirectory || options.defaultDirectory);
      if (mounted.current && selected) setDraft((value) => ({ ...value, workingDirectory: selected }));
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(null);
    }
  };
  const save = async () => {
    if (inFlight.current || blocked) return;
    inFlight.current = true;
    setBusy("saving"); setError(null);
    try {
      await options.onApply({ ...draft, workingDirectory: draft.workingDirectory || null });
      if (mounted.current) finish();
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(null);
    }
  };

  return <dialog ref={dialog} aria-modal="true" aria-label="Agent settings" aria-describedby={`${id}-description`}
    onCancel={(event) => { event.preventDefault(); dismiss(); }}
    onClick={(event) => {
      if (event.target !== event.currentTarget) return;
      const bounds = event.currentTarget.getBoundingClientRect();
      if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) dismiss();
    }}
    onKeyDown={(event) => {
      if (event.key !== "Tab") return;
      const controls = dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled):not([type="radio"]), input[type="radio"]:checked:not(:disabled)');
      const first = controls?.[0];
      const last = controls?.[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto max-h-[calc(100dvh-40px)] w-[520px] max-w-[calc(100%_-_40px)] overflow-y-auto rounded-[22px] border-0 p-0 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)] backdrop:backdrop-blur-[4px]">
    <form onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <header className="flex items-start gap-3.5 px-7 pt-7">
        <div className="shadow-glass-tile flex h-10 w-10 shrink-0 items-center justify-center rounded-[13px] bg-[var(--surface-tile)] text-[var(--color-text-secondary)]"><Bot size={20} aria-hidden="true" /></div>
        <div className="min-w-0 flex-1">
          <h2 className="text-[20px] font-medium tracking-tight">Agent mode</h2>
          <p id={`${id}-description`} className="mt-1 text-[13px] leading-5 text-[var(--color-text-secondary)]">Work with files and run commands on your computer.</p>
        </div>
        <button type="button" disabled={!!busy} aria-label="Close Agent settings" onClick={dismiss} className="-mr-1 -mt-1 rounded-lg p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] disabled:opacity-40"><X size={16} /></button>
      </header>

      <div className="space-y-5 px-7 py-6">
        <div className="flex items-center justify-between gap-4 rounded-[13px] bg-[var(--wash-chip)] px-4 py-3.5">
          <div><label id={`${id}-enable`} htmlFor={`${id}-toggle`} className="cursor-pointer text-[13px] font-medium">Enable agent mode</label><p className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">Only for this thread</p></div>
          <button ref={toggle} id={`${id}-toggle`} type="button" role="switch" aria-checked={draft.enabled} aria-labelledby={`${id}-enable`} disabled={blocked}
            onClick={() => setDraft((value) => ({ ...value, enabled: !value.enabled }))}
            className={`relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-[var(--ring-focus-visible)] disabled:opacity-40 ${draft.enabled ? "bg-brand dark:bg-[var(--color-cherry)]" : "bg-[var(--color-border-strong)]"}`}>
            <span aria-hidden="true" className={`absolute left-[3px] top-[3px] h-4 w-4 rounded-full shadow-sm transition-transform motion-reduce:transition-none ${draft.enabled ? "translate-x-4 bg-on-brand dark:bg-[var(--color-on-accent)]" : "bg-[var(--color-text-primary)]"}`} />
          </button>
        </div>

        <fieldset disabled={blocked}>
          <legend className="mb-2.5 text-[13px] font-medium text-[var(--color-text-secondary)]">Permissions</legend>
          <div className="grid grid-cols-2 gap-2.5">
            {PERMISSIONS.map(({ value, label, description, icon: Icon }) => <label key={value}
              className={`relative cursor-pointer rounded-[13px] border p-3.5 transition-colors focus-within:ring-2 focus-within:ring-[var(--ring-focus-visible)] ${blocked ? "cursor-default opacity-50" : "hover:bg-[var(--wash-chip-hover)]"} ${draft.permission === value ? "border-brand bg-brand/10 dark:border-[var(--color-border-accent)] dark:bg-[var(--wash-chip)]" : "border-[var(--color-border)] bg-[var(--wash-row)]"}`}>
              <input type="radio" className="sr-only" name={`${id}-permission`} aria-label={label} value={value} checked={draft.permission === value} onChange={() => setDraft((current) => ({ ...current, permission: value }))} />
              <span className="mb-3 flex items-center justify-between text-[var(--color-text-secondary)]"><Icon size={17} aria-hidden="true" /><span aria-hidden="true" className={`flex h-3.5 w-3.5 items-center justify-center rounded-full border ${draft.permission === value ? "border-brand bg-brand dark:border-[var(--color-text-primary)] dark:bg-transparent" : "border-[var(--color-border-strong)]"}`}>{draft.permission === value ? <span className="h-1.5 w-1.5 rounded-full bg-on-brand dark:bg-[var(--color-text-primary)]" /> : null}</span></span>
              <span className="block text-[13px] font-medium">{label}</span>
              <span className="mt-1 block text-[12px] leading-[18px] text-[var(--color-text-secondary)]">{description}</span>
            </label>)}
          </div>
        </fieldset>

        <section>
          <label htmlFor={`${id}-directory`} className="text-[13px] font-medium text-[var(--color-text-secondary)]">Working directory</label>
          <div className="mt-2.5 flex items-center gap-2 rounded-[11px] border border-[var(--color-border)] bg-[var(--wash-row)] p-1.5 pl-3 focus-within:border-[var(--color-border-accent)] focus-within:shadow-[0_0_0_3px_var(--ring-focus-halo)]">
            <FolderOpen size={16} aria-hidden="true" className="shrink-0 text-[var(--color-text-tertiary)]" />
            <input id={`${id}-directory`} value={draft.workingDirectory ?? ""} disabled={blocked}
              onChange={(event) => { setDraft((value) => ({ ...value, workingDirectory: event.target.value || null })); setError(null); }}
              placeholder={options.defaultDirectory || "Default thread folder"} title={draft.workingDirectory || options.defaultDirectory}
              aria-describedby={draft.workingDirectory ? undefined : `${id}-directory-help`} aria-invalid={!!error}
              className="min-w-0 flex-1 bg-transparent py-1.5 text-[12px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-tertiary)] disabled:opacity-50" />
            <button type="button" disabled={blocked} onClick={() => void choose()} aria-label="Choose working directory" className="shrink-0 rounded-[7px] bg-[var(--wash-chip)] px-2.5 py-1.5 text-[12px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-40">{busy === "choosing" ? "Opening…" : "Browse"}</button>
          </div>
          {!draft.workingDirectory ? <p id={`${id}-directory-help`} className="mt-2 text-[12px] leading-[18px] text-[var(--color-text-tertiary)]">A separate folder for this thread in Axiom’s local data.</p> : null}
          {draft.workingDirectory ? <button type="button" disabled={blocked} onClick={() => { setDraft((value) => ({ ...value, workingDirectory: null })); setError(null); }} className="mt-1 text-[12px] text-[var(--color-text-secondary)] underline decoration-[var(--color-border-accent)] underline-offset-4 hover:text-[var(--color-text-primary)] disabled:opacity-40">Use default thread folder</button> : null}
        </section>

        <p className="flex items-start gap-2 text-[12px] leading-[18px] text-[var(--color-text-tertiary)]"><Info size={13} className="mt-0.5 shrink-0" aria-hidden="true" /><span>Commands use your computer’s permissions and can access files and the network outside this folder.</span></p>
        {error || (options.disabledReason && !busy) ? <p role={error ? "alert" : "status"} className={`rounded-[10px] px-3 py-2.5 text-[12px] leading-5 ${error ? "bg-[var(--color-danger-soft)] text-[var(--color-danger-strong)]" : "bg-[var(--wash-chip)] text-[var(--color-text-secondary)]"}`}>{error || options.disabledReason}</p> : null}
      </div>

      <footer className="flex justify-end gap-2 border-t border-[var(--color-border)] px-7 py-4">
        <button type="button" disabled={!!busy} onClick={dismiss} className="rounded-[9px] bg-[var(--wash-chip)] px-4 py-2 text-[13px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-40">Cancel</button>
        <button type="submit" disabled={blocked} className="flex items-center gap-2 rounded-[9px] bg-[var(--color-cherry)] px-4 py-2 text-[13px] font-medium text-on-accent transition-colors hover:bg-[var(--color-cherry-bright)] disabled:opacity-40">{busy === "saving" ? <LoaderCircle size={14} className="animate-spin" aria-hidden="true" /> : null}{busy === "saving" ? "Saving…" : "Save changes"}</button>
      </footer>
    </form>
  </dialog>;
}

/** @jsxRuntime automatic */
import { Bot, FolderOpen, Info, LoaderCircle, Power, ShieldCheck, Terminal, X } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState } from "react";
import type { AgentControlOptions, AgentSelection } from "./AgentModeControl";
import { McpToolSelection } from "./McpToolSelection";

const ACCESS_LEVELS = [
  { value: "off", label: "Off", description: "No local commands or file changes.", icon: Power },
  { value: "approve_commands", label: "Ask for permission", description: "Ask before each local tool action.", icon: ShieldCheck },
  { value: "full_access", label: "Full access", description: "Run local tools without asking.", icon: Terminal },
] as const;

function sameSelection(left: AgentSelection, right: AgentSelection): boolean {
  return left.enabled === right.enabled && left.permission === right.permission
    && (left.workingDirectory || null) === (right.workingDirectory || null)
    && JSON.stringify([...(left.mcpTools ?? [])].sort()) === JSON.stringify([...(right.mcpTools ?? [])].sort());
}

/** Renderer-owned modal; no OS prompt or composer-positioned overlay. */
export function AgentSettingsDialog({ options, onClose }: { options: AgentControlOptions; onClose(): void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const selectedLevel = useRef<HTMLInputElement>(null);
  const mounted = useRef(false);
  const inFlight = useRef(false);
  const attemptedSave = useRef(false);
  const [draft, setDraft] = useState(() => ({ ...options.selection, mcpTools: [...(options.selection.mcpTools ?? [])] }));
  const original = useRef(draft);
  const [busy, setBusy] = useState<"saving" | "choosing" | null>(null);
  const [error, setError] = useState<{ kind: "save" | "directory"; message: string } | null>(null);
  const id = useId();
  const blocked = !!busy || !!options.disabledReason;
  const level = draft.enabled ? draft.permission : "off";

  useLayoutEffect(() => {
    mounted.current = true;
    const element = dialog.current!;
    element.showModal();
    if (!selectedLevel.current?.disabled) selectedLevel.current?.focus({ preventScroll: true });
    return () => { mounted.current = false; element.close(); };
  }, []);

  const finish = () => {
    dialog.current?.close();
    onClose();
  };
  const choose = async () => {
    if (inFlight.current || blocked) return;
    inFlight.current = true;
    setBusy("choosing"); setError(null);
    try {
      const selected = await options.onChooseDirectory(draft.workingDirectory || options.defaultDirectory);
      if (mounted.current && selected) setDraft((value) => ({ ...value, workingDirectory: selected }));
    } catch (cause) {
      if (mounted.current) setError({ kind: "directory", message: cause instanceof Error ? cause.message : String(cause) });
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(null);
    }
  };
  const dismiss = async () => {
    if (inFlight.current) return;
    // MCP may save before the Agent write fails. After an attempt, compare to
    // the host's acknowledged state so reverting a draft can undo that save.
    const baseline = attemptedSave.current ? options.selection : original.current;
    if (sameSelection(draft, baseline)) { finish(); return; }
    if (options.disabledReason) { setError({ kind: "save", message: options.disabledReason }); return; }
    if (draft.mcpTools.length > 256) { setError({ kind: "save", message: "Select up to 256 tools for this conversation." }); return; }
    inFlight.current = true;
    setBusy("saving"); setError(null);
    try {
      attemptedSave.current = true;
      await options.onApply({ ...draft, workingDirectory: draft.workingDirectory || null });
      if (mounted.current) finish();
    } catch (cause) {
      if (mounted.current) setError({ kind: "save", message: cause instanceof Error ? cause.message : String(cause) });
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(null);
    }
  };

  return <dialog ref={dialog} aria-modal="true" aria-label="Agent settings" aria-describedby={`${id}-description`} aria-busy={busy === "saving" || undefined}
    onCancel={(event) => { event.preventDefault(); void dismiss(); }}
    onClick={(event) => {
      if (event.target !== event.currentTarget) return;
      const bounds = event.currentTarget.getBoundingClientRect();
      if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) void dismiss();
    }}
    onKeyDown={(event) => {
      if (event.key !== "Tab") return;
      const controls = dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled):not([type="radio"]), input[type="radio"]:checked:not(:disabled)');
      const first = controls?.[0];
      const last = controls?.[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto max-h-[calc(100dvh-40px)] w-[520px] max-w-[calc(100%_-_40px)] overflow-hidden rounded-[22px] border-0 p-0 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)] backdrop:backdrop-blur-[4px]">
    <form className="flex max-h-[calc(100dvh-40px)] flex-col" onSubmit={(event) => { event.preventDefault(); void dismiss(); }}>
      <header className="flex shrink-0 items-start gap-3.5 px-7 pt-7">
        <div className="shadow-glass-tile flex h-10 w-10 shrink-0 items-center justify-center rounded-[13px] bg-[var(--surface-tile)] text-[var(--color-text-secondary)]"><Bot size={20} aria-hidden="true" /></div>
        <div className="min-w-0 flex-1">
          <h2 className="text-[20px] font-medium tracking-tight">Agent mode</h2>
          <p id={`${id}-description`} className="mt-1 text-[13px] leading-5 text-[var(--color-text-secondary)]">Set permissions and choose connections for this thread.</p>
        </div>
        <button type="button" disabled={!!busy} aria-label="Close Agent settings" onClick={() => void dismiss()} className="-mr-1 -mt-1 rounded-lg p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] disabled:opacity-40"><X size={16} /></button>
      </header>

      <div className="min-h-0 space-y-5 overflow-y-auto px-7 py-6">
        <fieldset disabled={blocked}>
          <legend className="mb-2.5 text-[13px] font-medium text-[var(--color-text-secondary)]">Access level <span className="ml-1 font-normal text-[var(--color-text-tertiary)]">· This thread</span></legend>
          <div className="grid grid-cols-3 gap-2.5">
            {ACCESS_LEVELS.map(({ value, label, description, icon: Icon }) => <label key={value}
              className={`relative cursor-pointer rounded-[13px] border p-3.5 transition-colors focus-within:ring-2 focus-within:ring-[var(--ring-focus-visible)] ${blocked ? "cursor-default opacity-50" : "hover:bg-[var(--wash-chip-hover)]"} ${level === value ? "border-brand bg-brand/10 dark:border-[var(--color-border-accent)] dark:bg-[var(--wash-chip)]" : "border-[var(--color-border)] bg-[var(--wash-row)]"}`}>
              <input ref={level === value ? selectedLevel : undefined} type="radio" className="sr-only" name={`${id}-access`} aria-label={label} value={value} checked={level === value}
                onChange={() => { setDraft(current => ({ ...current, enabled: value !== "off", permission: value === "full_access" ? "full_access" : "approve_commands" })); setError(null); }} />
              <span className="mb-3 flex items-center justify-between text-[var(--color-text-secondary)]"><Icon size={17} aria-hidden="true" /><span aria-hidden="true" className={`flex h-3.5 w-3.5 items-center justify-center rounded-full border ${level === value ? "border-brand bg-brand dark:border-[var(--color-text-primary)] dark:bg-transparent" : "border-[var(--color-border-strong)]"}`}>{level === value ? <span className="h-1.5 w-1.5 rounded-full bg-on-brand dark:bg-[var(--color-text-primary)]" /> : null}</span></span>
              <span className="block min-h-[34px] text-[13px] font-medium leading-[17px]">{label}</span>
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
        <McpToolSelection servers={options.mcpServers} error={options.mcpError} selected={draft.mcpTools ?? []} disabled={blocked}
          onChange={mcpTools => { setDraft(value => ({ ...value, mcpTools })); setError(null); }} />
      </div>

      <footer className="shrink-0 space-y-3 border-t border-[var(--color-border)] px-7 py-4 text-[12px] leading-5">
        {error ? <div role="alert" className="rounded-[10px] bg-[var(--color-danger-soft)] px-3 py-2.5 text-[var(--color-danger-strong)]">
          {error.kind === "save" ? "Changes couldn’t be saved. " : "Couldn’t choose a folder. "}{error.message}
        </div> : <p role="status" className="flex items-center gap-2 text-[var(--color-text-tertiary)]">{busy ? <LoaderCircle size={13} aria-hidden="true" className="animate-spin" /> : null}{busy === "saving" ? "Saving changes…" : busy === "choosing" ? "Waiting for folder selection…" : "Changes save when you close."}</p>}
        {error?.kind === "save" ? <div className="flex justify-end gap-2">
          <button type="button" disabled={!!busy} onClick={finish} className="rounded-[9px] bg-[var(--wash-chip)] px-4 py-2 text-[var(--color-text-secondary)] hover:bg-[var(--wash-chip-hover)] disabled:opacity-40">Discard changes</button>
          <button type="button" disabled={blocked} onClick={() => void dismiss()} className="rounded-[9px] bg-[var(--color-cherry)] px-4 py-2 font-medium text-on-accent hover:bg-[var(--color-cherry-bright)] disabled:opacity-40">Try again</button>
        </div> : null}
        {options.disabledReason && !busy && !error ? <p role="status" className="text-[var(--color-text-secondary)]">{options.disabledReason}</p> : null}
      </footer>
    </form>
  </dialog>;
}

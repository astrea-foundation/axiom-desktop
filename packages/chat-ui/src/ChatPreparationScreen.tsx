/** @jsxRuntime automatic */
import { Check, CircleAlert, LoaderCircle, LockKeyhole, RefreshCw, Settings, ShieldCheck } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { AccountPresentation } from "@axiom/chat-core/account";
import { AxiomMark } from "./AxiomLogo";

type Preparation = Extract<AccountPresentation, { kind: "starting" | "unavailable" }>;
type Phase = NonNullable<Preparation["phase"]> | "starting";
const steps: Record<Phase, string> = {
  starting: "Start Axiom",
  checking_sign_in: "Check sign-in",
  switching_account: "Update account",
  opening_chats: "Unlock saved key",
  decrypting_chats: "Decrypt chats",
};

function observePhase(previous: Phase[], phase: Phase): Phase[] {
  // Replace the generic connecting state once a specific native stage arrives.
  const observed: Phase[] = [...previous.filter(step => step !== phase && (phase === "starting" || step !== "starting")), phase];
  return observed.slice(-4);
}

/** Replaces the composer using observed native stages; never simulates progress. */
export function ChatPreparationScreen({ account, onRetry, onOpenSettings }: {
  account: Preparation;
  onRetry: () => void | Promise<unknown>;
  onOpenSettings?: (trigger: HTMLElement) => void;
}) {
  const phase: Phase = account.phase ?? "starting";
  const failed = account.kind === "unavailable";
  const [history, setHistory] = useState<{ failed: boolean; steps: Phase[] }>({ failed, steps: [phase] });
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const root = useRef<HTMLElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const id = useId();
  const currentSteps = observePhase(history.failed && !failed ? [] : history.steps, phase);
  useEffect(() => {
    setHistory(current => ({ failed, steps: observePhase(current.failed && !failed ? [] : current.steps, phase) }));
  }, [phase, failed]);
  useEffect(() => {
    if (failed && (document.activeElement === document.body || root.current?.contains(document.activeElement))) {
      heading.current?.focus({ preventScroll: true });
    }
  }, [failed]);
  async function retry() {
    if (inFlight.current) return;
    inFlight.current = true;
    setRetrying(true); setRetryError(null);
    try { await onRetry(); }
    catch (cause) { setRetryError(cause instanceof Error ? cause.message : String(cause)); }
    finally { inFlight.current = false; setRetrying(false); }
  }
  return <section ref={root} data-chat-preparation aria-label="Chat preparation" aria-describedby={`${id}-description`}
    className="flex w-full min-w-0 flex-col items-center text-center">
    <AxiomMark className="mb-6 h-7 w-auto shrink-0" />
    <div role="status" aria-live="polite" aria-atomic="true"><h1 ref={heading} id={id} tabIndex={-1} className="text-[clamp(28px,4.2vw,44px)] font-normal leading-[1.08] tracking-[-0.035em] text-[var(--color-text-primary)] outline-none"
      style={{ fontFamily: "var(--font-display)" }}>
      {account.title.replace(/…$/, "")}
    </h1></div>
    <p id={`${id}-description`} role={failed ? "alert" : undefined} className="mt-3 max-w-[52ch] text-[14px] leading-relaxed text-[var(--color-text-secondary)]">{account.detail}</p>
    <div aria-busy={!failed || retrying} className="chat-preparation-surface shadow-glass-card relative mt-6 flex min-h-[168px] w-full flex-col justify-center overflow-hidden rounded-[22px] border border-[var(--color-border)] bg-[var(--surface-card)] px-5 py-6 text-left sm:px-6">
      <ol aria-label="Observed preparation steps" className="space-y-4">
        {currentSteps.map(step => {
          const active = step === phase;
          const encrypted = step === "opening_chats" || step === "decrypting_chats";
          const Icon = !active ? Check : failed ? CircleAlert : encrypted ? LockKeyhole : LoaderCircle;
          return <li key={step} aria-current={active ? "step" : undefined} className={`flex min-w-0 items-center gap-3 text-[13px] ${active ? "text-[var(--color-text-primary)]" : "text-[var(--color-text-secondary)]"}`}>
            <Icon size={18} strokeWidth={1.7} aria-hidden="true" className={`shrink-0 ${active && !failed ? encrypted ? "chat-preparation-pulse text-brand" : "animate-spin text-brand motion-reduce:animate-none" : ""}`} />
            <span className="min-w-0 flex-1">{steps[step]}</span>
            <span className="shrink-0 text-[11.5px] text-[var(--color-text-secondary)]">{active ? failed ? "Didn’t finish" : "In progress" : "Done"}</span>
          </li>;
        })}
      </ol>
      {failed ? <>
        {account.technicalDetails ? <details className="mt-5 text-[12px] text-[var(--color-text-secondary)]">
          <summary className="cursor-pointer">Details</summary><p className="mt-2 whitespace-pre-wrap break-words">{account.technicalDetails}</p>
        </details> : null}
        {retryError ? <p role="alert" className="mt-3 text-[12px] text-[var(--color-danger-strong)]">{retryError}</p> : null}
        <div className="mt-5 flex flex-wrap items-center gap-2">
          <button type="button" disabled={retrying} onClick={() => void retry()} className="inline-flex items-center gap-2 rounded-full bg-brand px-4 py-2 text-[12px] font-medium text-on-brand hover:bg-brand-hover disabled:opacity-50">
            <RefreshCw size={13} aria-hidden="true" className={retrying ? "animate-spin motion-reduce:animate-none" : ""} />{retrying ? "Trying again…" : "Try again"}
          </button>
          {onOpenSettings ? <button type="button" onClick={event => onOpenSettings(event.currentTarget)} className="inline-flex items-center gap-2 rounded-full bg-[var(--wash-chip)] px-4 py-2 text-[12px] text-[var(--color-text-primary)] hover:bg-[var(--wash-chip-hover)]"><Settings size={13} aria-hidden="true" />Open Settings</button> : null}
        </div>
      </> : <div className="chat-preparation-activity absolute inset-x-0 bottom-0 h-0.5 overflow-hidden bg-[var(--wash-chip)]" aria-hidden="true"><span /></div>}
    </div>
    {!failed ? <p className="mt-4 inline-flex items-center justify-center gap-1.5 text-[11.5px] text-[var(--color-text-secondary)]"><ShieldCheck size={12} aria-hidden="true" className="shrink-0" />Your saved chats stay encrypted on this device.</p> : null}
  </section>;
}

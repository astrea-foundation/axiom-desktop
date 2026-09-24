import { Check, RotateCcw, Shield, X } from "lucide-react";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { ProviderModel } from "../types";
import { PROOF_IDLE_AFTER_MS, proofBadge, proofState } from "../privacyProof";
import { PrivacyProofDialog } from "./PrivacyProofDialog";

type TeeVerificationBadgeProps = {
  session: ClientSessionState;
  model?: ProviderModel;
  modelId: string;
  webEnabled: boolean;
  disabledReason?: string;
  onVerify: () => void | Promise<unknown>;
};

export function TeeVerificationBadge({ session, model, modelId, webEnabled, disabledReason, onVerify }: TeeVerificationBadgeProps) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const mounted = useRef(false);
  const lastActivityAt = useRef(Date.now());
  const [, wakeActivity] = useReducer((version: number) => version + 1, 0);
  const running = useRef(session.running);
  useEffect(() => {
    // A running reply is activity, but verification/retry timers are not.
    if (session.running || running.current) lastActivityAt.current = Date.now();
    running.current = session.running;
    setNow(Date.now());
  }, [session.running]);
  useEffect(() => {
    mounted.current = true;
    const updateClock = () => {
      const time = Date.now();
      if (running.current) lastActivityAt.current = time;
      setNow(time);
    };
    const recordActivity = () => {
      const time = Date.now();
      const wasIdle = time - lastActivityAt.current >= PROOF_IDLE_AFTER_MS;
      lastActivityAt.current = time;
      if (wasIdle) { setNow(time); wakeActivity(); }
    };
    const wake = () => { recordActivity(); updateClock(); };
    const visibilityChanged = () => { if (!document.hidden) wake(); else updateClock(); };
    const activityEvents = ["keydown", "pointerdown", "pointermove", "wheel"] as const;
    const timer = setInterval(updateClock, 1000);
    for (const event of activityEvents) document.addEventListener(event, recordActivity, { capture: true, passive: true });
    window.addEventListener("focus", wake);
    window.addEventListener("online", updateClock);
    document.addEventListener("visibilitychange", visibilityChanged);
    return () => {
      mounted.current = false;
      clearInterval(timer);
      for (const event of activityEvents) document.removeEventListener(event, recordActivity, true);
      window.removeEventListener("focus", wake);
      window.removeEventListener("online", updateClock);
      document.removeEventListener("visibilitychange", visibilityChanged);
    };
  }, []);
  const currentProofState = refreshing ? "verifying" : proofState(session, modelId, now);
  const idle = !session.running && now - lastActivityAt.current >= PROOF_IDLE_AFTER_MS;
  const state = idle && !["development", "outdated", "degraded"].includes(currentProofState) ? "idle" : currentProofState;
  const badge = proofBadge(session, state);
  const verifying = currentProofState === "verifying";
  const reason = disabledReason || (session.needsResync ? "Wait for the thread to reconnect" : session.running ? "Wait for the current reply to finish" : verifying ? "Verification in progress" : undefined);
  const color = badge.tone === "muted" ? "var(--color-text-muted)" : `var(--color-${badge.tone})`;
  const refresh = useCallback(async () => {
    if (!mounted.current || busy.current || reason) return;
    busy.current = true; setRefreshing(true); setError(null);
    try { await onVerify(); }
    catch {
      if (mounted.current) setError("Couldn’t refresh the verification report. Try again.");
    }
    finally {
      busy.current = false;
      if (mounted.current) { setRefreshing(false); setNow(Date.now()); }
    }
  }, [onVerify, reason]);

  return <>
    <div className="inline-flex h-7 shrink-0 items-center rounded-full bg-[var(--surface-tile)] text-[11.5px] font-medium tracking-[-0.01em] text-[var(--color-text-secondary)] shadow-[0_0_0_1px_var(--ring-hairline),0_1px_2px_var(--shade-xs)]">
      <button type="button" onClick={() => setOpen(true)} aria-haspopup="dialog" aria-expanded={open}
        aria-label={`${badge.label}. View privacy proof`} title="View privacy proof"
        className="inline-flex h-full items-center gap-1.5 rounded-l-full pl-2 pr-2 transition-colors hover:bg-[var(--wash-chip-hover)]">
        <span className="relative flex h-4 w-4 items-center justify-center" aria-hidden="true">
          <Shield size={15} strokeWidth={0} fill={color} />
          {badge.tone === "success" && !session.running ? <Check size={8} strokeWidth={3.5} className="absolute text-on-accent" /> : badge.tone === "danger" ? <X size={8} strokeWidth={3.5} className="absolute text-on-accent" /> : null}
        </span>
        <span aria-live="polite">{badge.label}</span>
      </button>
      <span aria-hidden="true" className="h-3 w-px bg-[var(--color-border)]" />
      <button type="button" onClick={() => void refresh()} disabled={!!reason} aria-label="Refresh TEE verification" title={reason ?? "Refresh TEE verification"}
        className="flex h-7 w-7 items-center justify-center rounded-r-full text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:opacity-50">
        <RotateCcw size={12} aria-hidden="true" className={verifying ? "animate-spin motion-reduce:animate-none" : ""} />
      </button>
    </div>
    {open ? <PrivacyProofDialog session={session} model={model} modelId={modelId} state={state} now={now} webEnabled={webEnabled}
      error={error ?? session.securityVerificationError ?? null} disabledReason={reason} onRefresh={() => void refresh()} onClose={() => setOpen(false)} /> : null}
  </>;
}

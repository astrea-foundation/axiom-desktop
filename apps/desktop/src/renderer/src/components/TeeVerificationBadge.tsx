import { Check, RotateCcw, Shield, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { ProviderModel } from "../types";
import { proofLabels, proofState } from "../privacyProof";
import { PrivacyProofDialog } from "./PrivacyProofDialog";

const REFRESH_BEFORE_EXPIRY_MS = 60_000;
const MIN_REFRESH_INTERVAL_MS = 30_000;
const MAX_RETRY_INTERVAL_MS = 300_000;

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
  const nextAutomaticAttemptAt = useRef(0);
  const failures = useRef(0);
  useEffect(() => {
    mounted.current = true;
    const updateClock = () => setNow(Date.now());
    const timer = setInterval(updateClock, 1000);
    window.addEventListener("focus", updateClock);
    window.addEventListener("online", updateClock);
    document.addEventListener("visibilitychange", updateClock);
    return () => {
      mounted.current = false;
      clearInterval(timer);
      window.removeEventListener("focus", updateClock);
      window.removeEventListener("online", updateClock);
      document.removeEventListener("visibilitychange", updateClock);
    };
  }, []);
  const state = refreshing ? "verifying" : proofState(session, modelId, now);
  const verifying = state === "verifying";
  const reason = disabledReason || (session.needsResync ? "Wait for the thread to reconnect" : session.running ? "Wait for the current reply to finish" : verifying ? "Verification in progress" : undefined);
  const color = state === "verified" ? "var(--color-success)" : state === "failed" ? "var(--color-danger)"
    : state === "expired" || state === "development" ? "var(--color-warning)" : "var(--color-text-muted)";
  const refresh = useCallback(async () => {
    if (!mounted.current || busy.current || reason) return;
    busy.current = true; setRefreshing(true); setError(null);
    try { await onVerify(); failures.current = 0; }
    catch {
      failures.current = Math.min(failures.current + 1, 5);
      if (mounted.current) setError("Couldn’t refresh the verification report. Retrying automatically.");
    }
    finally {
      // Also pace successful checks: a provider may return the same short lease.
      const delay = Math.min(MAX_RETRY_INTERVAL_MS, MIN_REFRESH_INTERVAL_MS * 2 ** Math.max(0, failures.current - 1));
      nextAutomaticAttemptAt.current = Date.now() + delay;
      busy.current = false;
      if (mounted.current) { setRefreshing(false); setNow(Date.now()); }
    }
  }, [onVerify, reason]);
  useEffect(() => {
    if (!modelId || reason || state === "development" || now < nextAutomaticAttemptAt.current) return;
    const evidence = session.securityEvidence;
    const expiresAt = evidence?.hardExpiresAtUnixSeconds;
    if (state === "verified" && evidence?.modelId === modelId && typeof expiresAt === "number" && Number.isFinite(expiresAt)
      && expiresAt * 1000 - now > REFRESH_BEFORE_EXPIRY_MS) return;
    // Uses the same native verifier as the button. Only a returned report can
    // renew the badge; the timer never extends evidence or marks it verified.
    void refresh();
  }, [modelId, now, reason, refresh, session.securityEvidence, state]);

  return <>
    <div className="inline-flex h-7 shrink-0 items-center rounded-full bg-[var(--surface-tile)] text-[11.5px] font-medium tracking-[-0.01em] text-[var(--color-text-secondary)] shadow-[0_0_0_1px_var(--ring-hairline),0_1px_2px_var(--shade-xs)]">
      <button type="button" onClick={() => setOpen(true)} aria-haspopup="dialog" aria-expanded={open}
        aria-label={`${proofLabels[state]}. View privacy proof`} title="View privacy proof"
        className="inline-flex h-full items-center gap-1.5 rounded-l-full pl-2 pr-2 transition-colors hover:bg-[var(--wash-chip-hover)]">
        <span className="relative flex h-4 w-4 items-center justify-center" aria-hidden="true">
          <Shield size={15} strokeWidth={0} fill={color} />
          {state === "verified" ? <Check size={8} strokeWidth={3.5} className="absolute text-on-accent" /> : state === "failed" ? <X size={8} strokeWidth={3.5} className="absolute text-on-accent" /> : null}
        </span>
        <span aria-live="polite">{proofLabels[state]}</span>
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

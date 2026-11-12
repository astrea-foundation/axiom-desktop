/** @jsxRuntime automatic */
import { Check, RotateCcw, Shield, X } from "lucide-react";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import type { ClientSessionState, DesktopMcpServer } from "@axiom/chat-core/viewTypes";
import type { ProviderModel } from "@axiom/chat-core/types";
import { PROOF_IDLE_AFTER_MS, PROOF_REFRESH_MAX_RETRY_MS, PROOF_REFRESH_RETRY_MS, proofBadge, proofRefreshDue, proofState } from "./privacyProof";
import { PrivacyProofDialog } from "./PrivacyProofDialog";

type TeeVerificationBadgeProps = {
  session: ClientSessionState;
  model?: ProviderModel;
  modelId: string;
  webEnabled: boolean;
  mcpServers?: DesktopMcpServer[];
  selectedMcpTools?: string[];
  disabledReason?: string;
  onVerify: () => void | Promise<unknown>;
  onEnsure?: () => void | Promise<unknown>;
};

export function TeeVerificationBadge({ session, model, modelId, webEnabled, mcpServers, selectedMcpTools, disabledReason, onVerify, onEnsure }: TeeVerificationBadgeProps) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const refreshMode = useRef<"automatic" | "manual">("manual");
  const nextRefresh = useRef(0);
  const refreshFailures = useRef(0);
  const mounted = useRef(false);
  const lastActivityAt = useRef(Date.now());
  const [activityVersion, wakeActivity] = useReducer((version: number) => version + 1, 0);
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
    const wake = () => { recordActivity(); updateClock(); wakeActivity(); };
    const online = () => { updateClock(); wakeActivity(); };
    const visibilityChanged = () => { if (!document.hidden) wake(); else updateClock(); };
    const activityEvents = ["keydown", "pointerdown", "pointermove", "wheel"] as const;
    const timer = setInterval(updateClock, 1000);
    for (const event of activityEvents) document.addEventListener(event, recordActivity, { capture: true, passive: true });
    window.addEventListener("focus", wake);
    window.addEventListener("online", online);
    document.addEventListener("visibilitychange", visibilityChanged);
    return () => {
      mounted.current = false;
      clearInterval(timer);
      for (const event of activityEvents) document.removeEventListener(event, recordActivity, true);
      window.removeEventListener("focus", wake);
      window.removeEventListener("online", online);
      document.removeEventListener("visibilitychange", visibilityChanged);
    };
  }, []);
  const reportState = proofState(session, modelId, now);
  const keepValidReport = refreshMode.current === "automatic" && (reportState === "verified" || reportState === "degraded");
  const currentProofState = refreshing && !keepValidReport ? "verifying" : reportState;
  const idle = !session.running && now - lastActivityAt.current >= PROOF_IDLE_AFTER_MS;
  const state = idle && !["development", "outdated", "degraded"].includes(currentProofState) ? "idle" : currentProofState;
  const badge = proofBadge(session, state);
  const verifying = currentProofState === "verifying";
  const reason = disabledReason || (session.needsResync ? "Wait for the thread to reconnect" : session.running ? "Wait for the current reply to finish" : refreshing || verifying ? "Verification in progress" : undefined);
  const color = badge.tone === "muted" ? "var(--color-text-muted)" : `var(--color-${badge.tone})`;
  const refresh = useCallback(async (mode: "automatic" | "manual" = "manual") => {
    if (!mounted.current || busy.current || reason) return;
    refreshMode.current = mode;
    busy.current = true; setRefreshing(true); setError(null);
    nextRefresh.current = Date.now() + PROOF_REFRESH_RETRY_MS;
    try {
      await (mode === "automatic" && onEnsure ? onEnsure() : onVerify());
      refreshFailures.current = 0;
      nextRefresh.current = Date.now() + PROOF_REFRESH_RETRY_MS;
    }
    catch {
      refreshFailures.current = Math.min(refreshFailures.current + 1, 5);
      nextRefresh.current = Date.now() + Math.min(PROOF_REFRESH_MAX_RETRY_MS,
        PROOF_REFRESH_RETRY_MS * 2 ** (refreshFailures.current - 1));
      if (mounted.current) setError("Couldn’t refresh the verification report. Try again.");
    }
    finally {
      busy.current = false;
      if (mounted.current) { setRefreshing(false); setNow(Date.now()); }
    }
  }, [onVerify, onEnsure, reason]);
  useEffect(() => {
    // Renew the selected environment, including managed providers, before its
    // report expires. Hidden/idle windows pause; streams finish before refresh.
    // An outdated TEE still needs explicit consent, never an automatic override.
    if (!modelId || idle || reason || refreshing || document.hidden || !navigator.onLine
      || currentProofState === "outdated" || (currentProofState === "failed" && !refreshFailures.current)) return;
    if (!proofRefreshDue(session, modelId, now) || now < nextRefresh.current) return;
    void refresh("automatic");
  }, [modelId, idle, reason, refreshing, currentProofState, session.securityEvidence, now, refresh, activityVersion]);

  return <>
    <div className="inline-flex h-7 shrink-0 items-center rounded-full bg-[var(--surface-tile)] text-[11.5px] font-medium tracking-[-0.01em] text-[var(--color-text-secondary)] shadow-[0_0_0_1px_var(--ring-hairline),0_1px_2px_var(--shade-xs)]">
      <button type="button" onClick={() => setOpen(true)} aria-haspopup="dialog" aria-expanded={open}
        aria-label={`${badge.label}. View privacy proof`} title={`Model inference: ${badge.label}. View privacy proof`}
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
    {open ? <PrivacyProofDialog session={session} model={model} modelId={modelId} state={state} now={now} webEnabled={webEnabled} mcpServers={mcpServers} selectedMcpTools={selectedMcpTools}
      error={error ?? session.securityVerificationError ?? null} disabledReason={reason} onRefresh={() => void refresh()} onClose={() => setOpen(false)} /> : null}
  </>;
}

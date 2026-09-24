import { ArrowLeft, Check, Copy, Power, RotateCcw, ShieldCheck } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { initialProxyState, type ProxyState } from "../../../shared/proxy";

const well = "shadow-glass-card relative rounded-2xl bg-[var(--surface-card)]";
const failureLabels: Record<string, string> = {
  Transient: "Connection interrupted", Authentication: "Sign-in required", LocalAuthentication: "Sign-in required",
  InsufficientCredit: "Insufficient credit", RateLimited: "Rate limit reached", ModelUnavailable: "Model unavailable",
  AttestationUnavailable: "Attestation unavailable", AttestationRejected: "Attestation rejected",
  SessionEstablishment: "Encrypted connection failed", Encryption: "Request encryption failed",
  Decryption: "Response verification failed", InvalidResponse: "Invalid provider response",
  Cancelled: "Request cancelled", InvalidRequest: "Unsupported request", CapabilityMismatch: "Unsupported request",
  Configuration: "Proxy configuration error",
};

export function ProxyView({ accountId, runtimeId, onInspecting }: {
  accountId: string | null;
  runtimeId: string | null;
  onInspecting?: (inspecting: boolean) => void;
}) {
  const [snapshot, setSnapshot] = useState<ProxyState>(initialProxyState);
  const [portDraft, setPortDraft] = useState("8484");
  const [copied, setCopied] = useState<"url" | "token" | null>(null);
  const [inspecting, setInspecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const operation = useRef(0);
  const api = window.axiomDesktop?.proxy;
  const state = snapshot.accountId === accountId ? snapshot : initialProxyState();
  const running = state.status === "running";
  const starting = state.status === "starting";
  const stopping = state.status === "stopping";
  const port = Number(portDraft);
  const portValid = /^\d+$/.test(portDraft) && Number.isInteger(port) && port >= 1 && port <= 65535;
  const dirty = port !== state.port;
  const available = !!api && !!accountId && !!runtimeId;
  const acceptState = (next: ProxyState) => setSnapshot((current) => next.revision >= current.revision ? next : current);

  useEffect(() => {
    if (!api) return;
    let disposed = false;
    const accept = (next: ProxyState) => { if (!disposed) acceptState(next); };
    const unsubscribe = api.onState(accept);
    void api.getState().then(accept).catch(() => { if (!disposed) setError("Could not read the proxy status. Restart Axiom and try again."); });
    return () => { disposed = true; unsubscribe(); };
  }, [api]);
  useEffect(() => { setPortDraft(String(state.port)); }, [state.port]);
  useEffect(() => {
    operation.current++;
    setPending(false); setError(null); setCopied(null);
  }, [accountId, runtimeId]);
  useEffect(() => () => { operation.current++; }, []);
  useEffect(() => {
    onInspecting?.(inspecting);
    return () => onInspecting?.(false);
  }, [inspecting, onInspecting]);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(null), 1_200);
    return () => clearTimeout(timer);
  }, [copied]);

  const run = async (action: () => Promise<unknown>) => {
    const id = ++operation.current;
    setPending(true); setError(null);
    try { await action(); }
    catch (failure) {
      if (id === operation.current) setError(failure instanceof Error ? failure.message.replace(/^Error invoking remote method '[^']+': (Error: )?/, "") : "The proxy action failed. Try again.");
    } finally { if (id === operation.current) setPending(false); }
  };
  const start = () => {
    if (!api || !accountId || !runtimeId || !portValid) return;
    void run(async () => { acceptState(await api.start(port, accountId, runtimeId)); });
  };
  const stop = () => { if (api) void run(async () => { acceptState(await api.stop()); }); };
  const copy = (which: "url" | "token") => {
    if (!api || !running || !accountId || !runtimeId) return;
    void run(async () => {
      if (which === "token") await api.copyToken(accountId, runtimeId);
      else if (state.baseUrl) await navigator.clipboard.writeText(state.baseUrl);
      setCopied(which);
    });
  };

  return (
    <div className="title-fade relative flex h-full min-h-0 flex-col overflow-y-auto">
      <div className="mx-auto flex w-full max-w-[760px] flex-1 flex-col gap-3 px-6 pb-8 pt-[88px]">
        {!api ? <p role="status">Restart Axiom to enable the proxy controls.</p> : !available ? <p role="status">Sign in and connect to start the local proxy.</p> : null}
        {error || state.error ? <p role="alert" className="text-[13px] text-[var(--color-danger)]">{error ?? state.error}</p> : null}
        {inspecting ? <>
          <button type="button" onClick={() => setInspecting(false)} className="mb-4 flex items-center gap-2 self-start text-[13px]"><ArrowLeft size={14} />Back</button>
          <div className={`${well} space-y-3 px-5 py-6`}>
            <h1 className="flex items-center gap-2 text-[22px]"><ShieldCheck size={22} />Attestation</h1>
            {state.evidence ? <>
              <p className="text-[13px] text-[var(--color-text-tertiary)]">Latest locally verified attestation, checked before sending encrypted model traffic.</p>
              <dl className="grid grid-cols-[auto_1fr] gap-x-5 gap-y-3 break-all text-[13px]">
                <dt>Model</dt><dd>{state.evidence.modelId}</dd>
                <dt>Provider</dt><dd>{state.evidence.providerId}</dd>
                <dt>Protocol</dt><dd>{state.evidence.protocol}</dd>
                <dt>Verified</dt><dd>{new Date(state.evidence.verifiedAt * 1000).toLocaleString()}</dd>
                <dt>Key fingerprint</dt><dd className="font-mono text-[12px]">{state.evidence.fingerprint}</dd>
                <dt>Response receipt</dt><dd>{state.evidence.responseVerified ? "Verified" : "No verified completion recorded for this request"}</dd>
              </dl>
            </> : <p className="text-[13px] text-[var(--color-text-tertiary)]">No attestation recorded in this proxy run. Evidence appears after a local client makes a model request.</p>}
          </div>
        </> : <>
          <div className={`${well} px-5 py-5`}>
            <div className="flex items-center justify-between gap-3">
              <div>
                <div role="status" className="flex items-center gap-2 text-[16px] font-medium"><span className={`h-1.5 w-1.5 rounded-full ${running ? "bg-[var(--color-success)]" : "bg-[var(--color-text-muted)]"}`} />{starting ? "Starting…" : stopping ? "Stopping…" : running ? "Running" : state.status === "failed" ? "Failed" : "Stopped"}</div>
                <p className="mt-1 text-[12.5px] text-[var(--color-text-tertiary)]">OpenAI-compatible endpoint for local clients</p>
              </div>
              <div className="flex gap-1">
                <button type="button" aria-label={running || starting ? "Stop proxy" : "Start proxy"} title={running || starting ? "Stop proxy" : "Start proxy"}
                  disabled={stopping || (!running && !starting && (!available || !portValid || pending))}
                  onClick={running || starting ? stop : start} className="rounded-full p-3 hover:bg-[var(--wash-row)] disabled:opacity-35"><Power size={16} /></button>
                <button type="button" aria-label="Restart proxy" title={dirty ? "Restart to apply port" : "Restart proxy"} disabled={!running || !portValid || pending || !available}
                  onClick={start} className="rounded-full p-3 hover:bg-[var(--wash-row)] disabled:opacity-35"><RotateCcw size={16} /></button>
              </div>
            </div>
            <button type="button" title="Copy URL" disabled={!running || pending} onClick={() => copy("url")} className="mt-5 flex w-full items-center gap-2 rounded-lg border border-[var(--color-border)] px-3.5 py-2.5 text-left disabled:opacity-50">
              <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{state.baseUrl ?? "Start the proxy to get its local URL"}</span>{copied === "url" ? <Check size={14} /> : <Copy size={14} />}
            </button>
          </div>
          <div className={`${well} px-5 py-3 text-[13px]`}>
            <label className="flex items-center gap-4">Port<input aria-label="Port" inputMode="numeric" value={portDraft} disabled={starting || stopping} onChange={(event) => setPortDraft(event.target.value.replace(/[^\d]/g, "").slice(0, 5))} className="min-w-0 flex-1 bg-transparent p-2 font-mono outline-none" /></label>
            {!portValid ? <p className="text-[12px] text-[var(--color-danger)]">Enter a port between 1 and 65535.</p> : dirty && running ? <p className="text-[12px] text-[var(--color-text-tertiary)]">Restart to apply the new port.</p> : null}
            <div className="mt-2 flex items-center justify-between"><span>Local token</span><button type="button" disabled={!running || pending} onClick={() => copy("token")} className="flex items-center gap-2 rounded-md p-2 hover:bg-[var(--wash-row)] disabled:opacity-35">{copied === "token" ? <Check size={14} /> : <Copy size={14} />}{copied === "token" ? "Copied" : "Copy token"}</button></div>
            <p className="mt-2 text-[12px] leading-5 text-[var(--color-text-tertiary)]">Use the URL and token in your local client. Each start creates a new token. Local HTTP traffic is plaintext on this machine; remote model traffic uses locally verified TEE attestation and E2EE. Signing out stops the proxy.</p>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className={`${well} px-4 py-3.5`}><div className="text-[11px] uppercase text-[var(--color-text-tertiary)]">Verified requests</div><div className="mt-1 font-mono text-[18px]">{state.completedRequests.toLocaleString()}</div></div>
            <div className={`${well} px-4 py-3.5`}><div className="text-[11px] uppercase text-[var(--color-text-tertiary)]">Tokens</div><div className="mt-1 font-mono text-[18px]">{state.totalTokens.toLocaleString()}</div><div className="mt-1 text-[11px] text-[var(--color-text-tertiary)]">Completed requests</div></div>
          </div>
          {state.errors.length ? <div className={`${well} px-4 py-3`}><h2 className="mb-2 text-[13px]">Recent errors</h2>{state.errors.map((entry, i) => <p key={`${entry.at}:${i}`} className="flex justify-between gap-3 py-1 text-[12px]"><span>{failureLabels[entry.kind] ?? "Request failed"}</span><time>{new Date(entry.at).toLocaleTimeString()}</time></p>)}</div> : null}
          <button type="button" onClick={() => setInspecting(true)} className={`${well} flex items-center gap-3 px-4 py-3.5 text-left hover:bg-[var(--surface-card-hover)]`}><ShieldCheck size={20} /><span><span className="block text-[13.5px]">Attestation</span><span className="block text-[12px] text-[var(--color-text-tertiary)]">Inspect evidence from this proxy run</span></span></button>
        </>}
      </div>
    </div>
  );
}

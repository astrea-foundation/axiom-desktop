import { Check, ChevronRight, CircleHelp, Copy, FileCode2, Globe2, Minus, RotateCcw, Shield, ShieldCheck, ShieldX, X } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { ProviderModel } from "../types";
import { proofGroups, proofLabels, replyProof, verificationAge, type ProofState } from "../privacyProof";

const explanation: Record<ProofState, string> = {
  idle: "Verification is paused after 30 minutes without activity. It resumes when you return; model traffic still requires fresh evidence.",
  verified: "Model messages are encrypted on your device to a key verified through hardware attestation.",
  outdated: "The provider’s TEE is missing security updates. Generation is paused until you accept this warning or choose another provider.",
  degraded: "You accepted this provider’s outdated TEE until Axiom restarts. Messages are still encrypted and authenticated, but the environment is missing security updates and may be vulnerable.",
  verifying: "Checking the model’s confidential-computing evidence on this device.",
  failed: "Verification failed. Model traffic requires valid evidence before it can be sent.",
  expired: "This cached report has expired. Axiom requires fresh evidence for new model requests. Ongoing replies still require authentication against the evidence accepted when each request started.",
  unavailable: "A current verification report isn’t available. Refresh to inspect the model’s proof.",
  unverified: "Check the model’s hardware evidence and encryption key before sending model messages.",
  development: "This development session has no production TEE proof.",
};

function VerifiedStatus() {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return;
    const dismissOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    const dismissEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    };
    document.addEventListener("pointerdown", dismissOutside);
    document.addEventListener("keydown", dismissEscape, true);
    return () => {
      document.removeEventListener("pointerdown", dismissOutside);
      document.removeEventListener("keydown", dismissEscape, true);
    };
  }, [open]);
  return <div ref={root} className="relative flex items-center gap-1.5"
    onMouseLeave={(event) => { if (!event.currentTarget.contains(document.activeElement)) setOpen(false); }}
    onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false); }}>
    <p role="status" className="text-[15px] font-medium">{proofLabels.verified}</p>
    <button type="button" aria-label="What does TEE verified mean?" aria-describedby={open ? id : undefined}
      onMouseEnter={() => setOpen(true)} onFocus={() => setOpen(true)} onClick={() => setOpen(true)}
      className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]">
      <CircleHelp size={14} aria-hidden="true" />
    </button>
    <div id={id} role="tooltip" aria-hidden={!open}
      className={`absolute left-0 top-full z-50 w-[340px] max-w-full pt-2 transition-[opacity,transform,visibility] duration-150 motion-reduce:transition-none ${open ? "visible translate-y-0 opacity-100" : "pointer-events-none invisible -translate-y-1 opacity-0"}`}>
      <div className="glass-panel-solid shadow-glass-pop rounded-xl border border-[var(--color-border)] p-3.5 text-[12px] leading-[19px] text-[var(--color-text-secondary)]">
        <p className="mb-1 font-medium text-[var(--color-text-primary)]">What does TEE verified mean?</p>
        <p>A TEE (Trusted Execution Environment) is a protected space on the provider’s servers. Axiom checks security evidence from the server’s hardware before sending your encrypted messages there. This helps keep your messages private while the AI processes them.</p>
      </div>
    </div>
  </div>;
}

function Disclosure({ title, value, children, verified = false }: {
  title: string; value?: string; children: ReactNode; verified?: boolean;
}) {
  return <details className="group/proof border-b border-[var(--color-border)] last:border-b-0">
    <summary className="flex cursor-pointer list-none items-center gap-2.5 rounded-lg px-3 py-3 text-[13px] transition-colors hover:bg-[var(--wash-row)] [&::-webkit-details-marker]:hidden">
      {verified ? <Check size={14} className="shrink-0 text-[var(--color-success)]" aria-hidden="true" /> : <Minus size={14} className="shrink-0 text-[var(--color-text-muted)]" aria-hidden="true" />}
      <span className="min-w-0 flex-1 font-medium">{title}</span>
      {value ? <span className="text-right text-[11.5px] text-[var(--color-text-tertiary)]">{value}</span> : null}
      <ChevronRight size={13} className="shrink-0 text-[var(--color-text-tertiary)] transition-transform group-open/proof:rotate-90 motion-reduce:transition-none" aria-hidden="true" />
    </summary>
    <div className="px-3 pb-4 pl-[38px] text-[12px] leading-[19px] text-[var(--color-text-secondary)]">{children}</div>
  </details>;
}

function Fact({ label, children, mono = false }: { label: string; children: ReactNode; mono?: boolean }) {
  return <div className="grid min-w-0 grid-cols-[100px_minmax(0,1fr)] gap-3 py-1.5">
    <dt className="text-[var(--color-text-tertiary)] [overflow-wrap:anywhere]">{label}</dt>
    <dd className={`m-0 min-w-0 select-text whitespace-pre-wrap [overflow-wrap:anywhere] ${mono ? "font-mono text-[11px]" : ""}`}>{children}</dd>
  </div>;
}

export function PrivacyProofDialog({ session, model, modelId, state, now, webEnabled, error, disabledReason, onRefresh, onClose }: {
  session: ClientSessionState;
  model?: ProviderModel;
  modelId: string;
  state: ProofState;
  now: number;
  webEnabled: boolean;
  error: string | null;
  disabledReason?: string;
  onRefresh: () => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const id = useId();
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  const mounted = useRef(false);
  // Reports belong to one selected model. Never relabel an old model’s report.
  const evidence = session.securityEvidence?.modelId === modelId ? session.securityEvidence : null;
  const providerLabel = evidence ? model?.providerId === evidence.providerId ? model.providerLabel : evidence.providerId : model?.providerLabel;
  const verified = state === "verified";
  const reply = replyProof(session);
  const groups = evidence ? proofGroups(evidence) : [];
  useLayoutEffect(() => {
    mounted.current = true;
    const opener = document.activeElement as HTMLElement | null;
    const element = dialog.current!;
    element.showModal();
    closeButton.current?.focus({ preventScroll: true });
    return () => {
      mounted.current = false;
      element.close();
      if (opener?.isConnected) opener.focus({ preventScroll: true });
    };
  }, []);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  const copyReport = async () => {
    if (!evidence) return;
    setCopyError(false);
    try {
      await navigator.clipboard.writeText(JSON.stringify({
        format: "axiom-verification-report-v1", reportStateAtExport: state,
        exportedAt: new Date().toISOString(), evidence,
      }, null, 2));
      if (mounted.current) setCopied(true);
    } catch { if (mounted.current) setCopyError(true); }
  };

  return <dialog ref={dialog} aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
    onCancel={(event) => { event.preventDefault(); onClose(); }}
    onKeyDown={(event) => {
      if (event.key !== "Tab") return;
      const controls = [...event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), summary, [tabindex="0"]')]
        .filter((element) => element.getClientRects().length > 0);
      const first = controls[0];
      const last = controls.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}
    onClick={(event) => {
      if (event.target !== event.currentTarget) return;
      const bounds = event.currentTarget.getBoundingClientRect();
      if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose();
    }}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto max-h-[calc(100dvh-40px)] w-[520px] max-w-[calc(100%_-_40px)] overflow-hidden rounded-[22px] border-0 p-0 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)] backdrop:backdrop-blur-[4px]">
    <div className="flex max-h-[calc(100dvh-40px)] flex-col motion-safe:animate-fade-in">
      <header className="flex shrink-0 items-center justify-between gap-3 px-6 pb-4 pt-6 sm:px-7 sm:pt-7">
        <h2 id={`${id}-title`} className="text-[20px] font-medium tracking-tight">Privacy proof</h2>
        <button ref={closeButton} type="button" aria-label="Close privacy proof" onClick={onClose}
          className="-mr-1 rounded-lg p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]"><X size={16} /></button>
      </header>
      <div className="min-h-0 overflow-y-auto px-6 pb-5 sm:px-7">
        <div className="-mx-3 mb-4">
          <Disclosure title="Latest reply verification" value={reply.label} verified={reply.verified}><p>{reply.detail}</p></Disclosure>
        </div>
        <h3 className="mb-3 text-[12px] font-medium text-[var(--color-text-secondary)]">Cached attestation report</h3>
        <div className="flex items-center gap-3">
          <div className={`shadow-glass-tile flex h-10 w-10 shrink-0 items-center justify-center rounded-[13px] bg-[var(--surface-tile)] ${verified ? "text-[var(--color-success)]" : state === "failed" ? "text-[var(--color-danger)]" : "text-[var(--color-text-tertiary)]"}`}>
            {verified ? <ShieldCheck size={21} aria-hidden="true" /> : state === "failed" ? <ShieldX size={21} aria-hidden="true" /> : <Shield size={21} aria-hidden="true" />}
          </div>
          <div className="min-w-0 flex-1">
            {verified ? <VerifiedStatus /> : <p role="status" className="text-[15px] font-medium">{proofLabels[state]}</p>}
            <p className="mt-0.5 flex flex-wrap gap-x-3 text-[12px] text-[var(--color-text-secondary)] [overflow-wrap:anywhere]"><span>{model?.shortLabel ?? modelId}</span>{providerLabel ? <span>{providerLabel}</span> : null}</p>
          </div>
        </div>
        <p id={`${id}-description`} className="mb-2 mt-4 text-[13px] leading-[21px] text-[var(--color-text-secondary)]">{explanation[state]}</p>
        {evidence ? <p className="mb-4 text-[11.5px] text-[var(--color-text-tertiary)]">
          {verified || state === "degraded" ? "Report checked locally" : "Previous report checked"}{" "}
          <time dateTime={new Date(evidence.verifiedAtUnixSeconds * 1000).toISOString()} title={new Date(evidence.verifiedAtUnixSeconds * 1000).toLocaleString()}>{verificationAge(evidence.verifiedAtUnixSeconds, now)}</time>
        </p> : null}
        {evidence ? <div className="-mx-3">
          {groups.map((group) => <Disclosure key={group.title} title={group.title} verified={verified && group.passed}
            value={group.passed ? verified ? "Verified" : "Passed at check" : "Not established"}>
            <p className="mb-2">{group.explanation}</p>
            {group.checks.map((check) => <div key={check.name} className="flex items-start justify-between gap-3 py-1"><span>{check.name}</span><span className="min-w-0 text-right text-[var(--color-text-tertiary)] [overflow-wrap:anywhere]">{check.detail}</span></div>)}
            {group.title === "Freshness" ? <p className="mt-2">{evidence.hardExpiresAtUnixSeconds * 1000 <= now ? "Expired" : "Expires"} {new Date(evidence.hardExpiresAtUnixSeconds * 1000).toLocaleString()}</p> : null}
          </Disclosure>)}
        </div> : <p className="mb-3 mt-4 rounded-[11px] bg-[var(--wash-row)] px-3 py-3 text-[12px] text-[var(--color-text-tertiary)]">{state === "verifying" ? "The report will appear when verification finishes." : "No report available for this model."}</p>}
        {evidence ? <details className="group/technical mt-3 border-t border-[var(--color-border)] pt-3">
          <summary className="flex cursor-pointer list-none items-center gap-2 rounded-lg py-1.5 text-[12px] text-[var(--color-text-secondary)] [&::-webkit-details-marker]:hidden">
            <FileCode2 size={14} aria-hidden="true" /><span className="flex-1">Technical details</span><ChevronRight size={13} className="transition-transform group-open/technical:rotate-90 motion-reduce:transition-none" aria-hidden="true" />
          </summary>
          <div className="mt-3 space-y-4 text-[12px] leading-[19px]">
            <dl>
              <Fact label="Model" mono>{evidence.modelId}</Fact>
              <Fact label="Provider">{evidence.providerId}</Fact>
              <Fact label="Attestation" mono>{evidence.attestationProtocol}</Fact>
              <Fact label="Encryption" mono>{evidence.e2eeProtocol} (v{evidence.e2eeEncryptionVersion})</Fact>
              <Fact label="Trust policy" mono>{evidence.trustPolicyVersion}</Fact>
              {evidence.attestationGeneration != null ? <Fact label="Generation" mono>{evidence.attestationGeneration}</Fact> : null}
              <Fact label="Checked">{new Date(evidence.verifiedAtUnixSeconds * 1000).toLocaleString()}</Fact>
              <Fact label="Expires">{new Date(evidence.hardExpiresAtUnixSeconds * 1000).toLocaleString()}</Fact>
              <Fact label="Key fingerprint" mono>{evidence.modelKeyFingerprint}</Fact>
              {evidence.tlsSpkiFingerprint ? <Fact label="TLS fingerprint" mono>{evidence.tlsSpkiFingerprint}</Fact> : null}
            </dl>
            <section><h3 className="mb-1 font-medium">Local check results</h3>
              <dl>{evidence.checks.map((check, index) => <Fact key={index} label={check.name === "Signed receipt" ? "Receipt policy" : check.name}>{check.name === "Signed receipt" && check.detail === "required" ? "Required for each reply" : check.detail}</Fact>)}</dl>
            </section>
            {evidence.providerClaims.length ? <section><h3 className="font-medium">Evidence fields</h3>
              <p className="mb-2 mt-1 text-[11.5px] text-[var(--color-text-tertiary)]">Supplied with the report. The local checks above identify what was verified.</p>
              <dl>{evidence.providerClaims.map((claim, index) => <Fact key={index} label={claim.name.replaceAll("_", " ")} mono>{claim.value || "None"}</Fact>)}</dl>
            </section> : null}
            {evidence.workloadManifest ? <details><summary className="cursor-pointer py-1 font-medium">Workload manifest</summary>
              <pre tabIndex={0} aria-label="Workload manifest" className="mt-2 max-h-[240px] overflow-auto rounded-[10px] bg-[var(--wash-code)] p-3 font-mono text-[11px] leading-[18px]">{evidence.workloadManifest}</pre>
            </details> : null}
          </div>
        </details> : null}
        {webEnabled ? <p className="mt-4 flex items-start gap-2 text-[12px] leading-[18px] text-[var(--color-text-secondary)]"><Globe2 size={13} className="mt-0.5 shrink-0" aria-hidden="true" />Web search queries are shared with the search service and are not end-to-end encrypted.</p> : null}
        {error ? <p role="alert" className="mt-4 rounded-[10px] bg-[var(--color-danger-soft)] px-3 py-2.5 text-[12px] text-[var(--color-danger-strong)]">{error}</p> : null}
        {state === "failed" && session.security?.detail ? <details className="mt-3 text-[12px]"><summary className="cursor-pointer text-[var(--color-text-secondary)]">Failure details</summary><p className="mt-2 whitespace-pre-wrap text-[var(--color-text-tertiary)] [overflow-wrap:anywhere]">{session.security.detail}</p></details> : null}
      </div>
      <footer className="shrink-0 border-t border-[var(--color-border)] px-6 py-4 sm:px-7">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <button type="button" aria-label="Copy verification report" disabled={!evidence} onClick={() => void copyReport()} className="flex items-center gap-2 rounded-lg py-2 text-[12px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40">{copied ? <Check size={13} aria-hidden="true" /> : <Copy size={13} aria-hidden="true" />}<span role="status">{copied ? "Copied" : "Copy verification report"}</span></button>
          <button type="button" onClick={onRefresh} disabled={!!disabledReason} title={disabledReason} className="flex items-center gap-2 rounded-[9px] bg-[var(--wash-chip)] px-3.5 py-2 text-[12px] font-medium transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-40"><RotateCcw size={13} aria-hidden="true" className={state === "verifying" ? "animate-spin motion-reduce:animate-none" : ""} />{state === "verifying" ? "Checking…" : "Refresh"}</button>
        </div>
        {disabledReason && state !== "verifying" ? <p className="mt-1 text-right text-[11px] text-[var(--color-text-tertiary)]">{disabledReason}</p> : null}
        {copyError ? <p role="alert" className="mt-2 text-[12px] text-[var(--color-danger-strong)]">Couldn’t copy the report. Try again.</p> : null}
      </footer>
    </div>
  </dialog>;
}

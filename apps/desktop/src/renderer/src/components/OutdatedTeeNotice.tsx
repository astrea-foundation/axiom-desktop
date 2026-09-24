import { AlertTriangle } from "lucide-react";
import { useState } from "react";

export function OutdatedTeeNotice({ state, disabled, onContinue }: {
  state?: string; disabled: boolean; onContinue: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (state !== "outdated" && state !== "degraded" && !busy) return null;
  return <div role="status" data-outdated-tee-warning className="mb-3 flex items-start gap-2 rounded-xl border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-[12.5px] text-[var(--color-warning)]">
    <AlertTriangle size={16} aria-hidden="true" className="mt-0.5 shrink-0" />
    <div className="min-w-0">
      <p className="font-medium">The provider’s TEE is missing security updates.</p>
      {state === "degraded" ? <p className="mt-1">You chose to continue with this provider until Axiom restarts.</p> : <>
        <p className="mt-1">Messages remain encrypted, but the outdated environment may be vulnerable. Continue anyway?</p>
        <p className="mt-1">Your choice applies to this provider until Axiom restarts.</p>
        <button type="button" disabled={busy || disabled} className="mt-2 rounded-lg border border-current/30 px-3 py-1.5 font-medium hover:bg-amber-500/10 disabled:opacity-50"
          onClick={async () => {
            setBusy(true); setError(null);
            try { await onContinue(); } catch (reason) { setError(reason instanceof Error ? reason.message : "Could not verify the provider. Try again."); }
            finally { setBusy(false); }
          }}>{busy ? "Checking provider…" : "Continue generation"}</button>
      </>}
      {error ? <p role="alert" className="mt-2">{error}</p> : null}
    </div>
  </div>;
}

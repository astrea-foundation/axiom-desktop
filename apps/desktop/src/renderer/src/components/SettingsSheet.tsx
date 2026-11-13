import { LogIn, LogOut, RefreshCw, ShieldCheck, X } from "lucide-react";
import { useEffect } from "react";
import type { AccountStatus } from "@axiom/axiom-acp-client";

interface SettingsSheetProps {
  onClose: () => void;
  connected: boolean;
  account: AccountStatus | null;
  onRefreshAccount: () => void;
  onLogin: () => void;
  onLogout: () => void;
}

export function SettingsSheet({
  onClose,
  connected,
  account,
  onRefreshAccount,
  onLogin,
  onLogout,
}: SettingsSheetProps) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const signedIn = account?.state === "valid";
  const accountLabel = signedIn
    ? (account.keyName || account.keyId || "Connected Axiom account")
    : account?.state === "unavailable"
      ? (account.detail || "Account status unavailable")
      : account?.state === "invalid"
        ? (account.detail || "Your sign-in expired")
        : "Not signed in";

  return (
    <div className="absolute inset-0 z-50 flex items-start justify-center bg-black/55 px-6 pt-24 backdrop-blur-sm">
      <div role="dialog" aria-label="Settings" className="animate-panel-in w-full max-w-[440px] rounded-2xl border border-[var(--color-border)] bg-[var(--color-bg-surface)] p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-[16px] font-medium">Settings</h2>
          <button type="button" onClick={onClose} className="rounded-md p-1 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)]" aria-label="Close settings"><X size={16} /></button>
        </div>
        <div className="flex flex-col gap-3 text-[13.5px] text-[var(--color-text-secondary)]">
          <div className="flex items-center justify-between gap-3 rounded-xl bg-[var(--color-bg-input)] px-3 py-3">
            <span className="text-[var(--color-text-primary)]">Appearance</span>
            <span className="text-[var(--color-text-tertiary)]">Dark</span>
          </div>
          <div className="rounded-xl bg-[var(--color-bg-input)] px-3 py-3">
            <div className="flex items-center gap-2 text-[var(--color-text-primary)]"><ShieldCheck size={14} className="text-[var(--color-cherry)]" />Desktop safety</div>
            <div className="mt-1 text-[12px] text-[var(--color-text-tertiary)]">Web access is enabled; local files and commands stay unavailable.</div>
          </div>
          <div className="rounded-xl bg-[var(--color-bg-input)] px-3 py-3">
            <div className="flex items-center justify-between gap-2">
              <div className="min-w-0">
                <div className="text-[var(--color-text-primary)]">Account</div>
                <div className="mt-0.5 truncate text-[12px] text-[var(--color-text-tertiary)]">{connected ? accountLabel : "AxiomCLI disconnected"}</div>
              </div>
              <button type="button" onClick={onRefreshAccount} disabled={!connected} className="rounded-md p-1.5 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] disabled:opacity-35" aria-label="Refresh account"><RefreshCw size={14} /></button>
            </div>
            <button type="button" disabled={!connected} onClick={signedIn ? onLogout : onLogin} className="mt-3 flex items-center gap-2 rounded-lg border border-[var(--color-border)] px-3 py-1.5 text-[12.5px] text-[var(--color-text-primary)] disabled:opacity-35">
              {signedIn ? <LogOut size={13} /> : <LogIn size={13} />}
              {signedIn ? "Sign out" : account?.state === "invalid" ? "Sign in again" : "Sign in with browser"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

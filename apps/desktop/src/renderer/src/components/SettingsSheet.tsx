import { ArrowLeft, LogIn, LogOut, RefreshCw, ShieldCheck } from "lucide-react";
import { useEffect, type ReactNode } from "react";
import type { AccountStatus } from "@axiom/axiom-acp-client";

interface SettingsSheetProps {
  onClose: () => void;
  connected: boolean;
  account: AccountStatus | null;
  onRefreshAccount: () => void;
  onLogin: () => void;
  onLogout: () => void;
}

function SettingsCard({
  index,
  title,
  sub,
  children,
}: {
  index: number;
  title: string;
  sub: string;
  children: ReactNode;
}) {
  return (
    <section
      className="animate-panel-in rounded-2xl bg-white/70 shadow-glass-card"
      style={{ animationDelay: `${index * 40}ms` }}
    >
      <div className="px-6 pb-4 pt-5">
        <h2 className="text-[16px] font-semibold tracking-[-0.02em] text-[var(--color-text-primary)]">{title}</h2>
        <p className="mt-0.5 text-[13px] text-[var(--color-text-tertiary)]">{sub}</p>
      </div>
      <div className="h-px w-full bg-[var(--color-border)]" aria-hidden="true" />
      <div className="p-6">{children}</div>
    </section>
  );
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
    <div
      role="region"
      aria-label="Settings"
      className="light-canvas animate-fade-in fixed inset-0 z-50 overflow-y-auto outline-none"
    >
      <div className="drag-region absolute inset-x-0 top-0 h-11" aria-hidden="true" />
      <div className="mx-auto w-full max-w-[640px] px-6 pb-24">
        <div className="pt-12">
          <button
            type="button"
            onClick={onClose}
            className="ax-pill no-drag transition-[background-color,box-shadow,transform] duration-150 hover:bg-white hover:text-[var(--color-text-primary)] active:scale-[0.97]"
          >
            <ArrowLeft size={13} />
            Back
          </button>
        </div>

        <header className="mb-8 mt-10">
          <h1 className="text-[30px] font-semibold tracking-[-0.03em] text-[var(--color-text-primary)]">Settings</h1>
          <p className="mt-1.5 text-[13.5px] text-[var(--color-text-tertiary)]">
            Threads stay in AxiomCLI&rsquo;s encrypted desktop store.
          </p>
        </header>

        <div className="flex min-w-0 flex-col gap-6">
          <SettingsCard index={0} title="Appearance" sub="How the app looks.">
            <div className="flex items-center justify-between gap-3 text-[13.5px]">
              <span className="text-[var(--color-text-primary)]">Theme</span>
              <span className="text-[var(--color-text-tertiary)]">Light</span>
            </div>
          </SettingsCard>

          <SettingsCard index={1} title="Desktop safety" sub="What the agent may touch.">
            <div className="flex items-center gap-2 text-[13.5px] text-[var(--color-text-primary)]">
              <ShieldCheck size={14} className="text-[var(--color-cherry)]" />
              Web access is enabled; local files and commands stay unavailable.
            </div>
          </SettingsCard>

          <SettingsCard index={2} title="Account" sub="Login identity is shared with AxiomCLI.">
            <div className="flex items-center justify-between gap-2">
              <div className="min-w-0">
                <div className="text-[13.5px] text-[var(--color-text-primary)]">{connected ? accountLabel : "AxiomCLI disconnected"}</div>
                <div className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">
                  {signedIn ? "Signed in" : "Sign in to load models"}
                </div>
              </div>
              <button
                type="button"
                onClick={onRefreshAccount}
                disabled={!connected}
                className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)] disabled:opacity-35"
                aria-label="Refresh account"
              >
                <RefreshCw size={14} />
              </button>
            </div>
            <button
              type="button"
              disabled={!connected}
              onClick={signedIn ? onLogout : onLogin}
              className="mt-4 flex items-center gap-1.5 rounded-md border border-[var(--color-border)] bg-white/60 px-3 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-[background-color,border-color,color,transform] duration-150 hover:border-[var(--color-border-accent)] hover:bg-white hover:text-[var(--color-text-primary)] active:scale-[0.97] disabled:opacity-50"
            >
              {signedIn ? <LogOut size={13} /> : <LogIn size={13} />}
              {signedIn ? "Sign out" : account?.state === "invalid" ? "Sign in again" : "Sign in with browser"}
            </button>
          </SettingsCard>
        </div>
      </div>
    </div>
  );
}

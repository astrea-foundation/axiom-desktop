import { LaptopMinimal, LogIn, LogOut, RefreshCw } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { AccountStatus } from "@axiom/axiom-acp-client";
import { useTheme, type ThemePreference } from "../lib/theme";
import { AccountScreen } from "./AccountScreen";
import { SETTINGS_CATEGORIES, SettingsPanel, type SettingsCategory } from "./SettingsPanel";
import { UsagePanel } from "./UsagePanel";
import { ApiKeysPanel } from "./ApiKeysPanel";
import { UpdatesPanel } from "./UpdatesPanel";
import type { DesktopUpdates } from "../useDesktopUpdates";
import { accountAvatar } from "../lib/accountAvatar";

const THEME_OPTIONS: Array<{ value: ThemePreference; label: string }> = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
];

interface SettingsSheetProps {
  initialCategory?: SettingsCategory;
  updates?: DesktopUpdates;
  onClose: () => void;
  connected: boolean;
  account: AccountStatus | null;
  runtimeInstanceId?: string | null;
  onRefreshAccount: () => void;
  onLogin: () => void;
  onLogout: () => void;
  onManageAccount: () => void;
}

export function SettingsSheet({
  initialCategory = "appearance",
  updates,
  onClose,
  connected,
  account,
  runtimeInstanceId,
  onRefreshAccount,
  onLogin,
  onLogout,
  onManageAccount,
}: SettingsSheetProps) {
  const [category, setCategory] = useState<SettingsCategory>(initialCategory);
  const id = useId();
  const tabs = useRef<HTMLDivElement>(null);
  useEffect(() => {
    tabs.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]')?.focus({ preventScroll: true });
  }, []);
  const theme = useTheme();
  const signedIn = account?.state === "valid";
  const avatar = accountAvatar(account?.account?.displayName);
  const accountLabel = signedIn
    ? (account.account?.displayName || account.account?.verifiedEmail || "Connected Axiom account")
    : account?.state === "unavailable"
      ? (account.detail || "Account status unavailable")
      : account?.state === "expired"
        ? (account.detail || "Your session expired")
        : "Not signed in";
  const linkedMethods = account?.account?.linkedMethods?.map((method) => ({
    passkey: "Passkey",
    google: "Google",
    password: "Email and password",
    ethereum_wallet: "Ethereum wallet",
  })[method]) ?? [];
  return (
    <AccountScreen title="Settings" onClose={onClose} wide>
        <div className="shadow-glass-card grid min-h-0 flex-1 grid-cols-[140px_minmax(0,1fr)] overflow-hidden rounded-[22px] bg-[var(--surface-card)] sm:grid-cols-[200px_minmax(0,1fr)]">
          <aside className="flex min-h-0 flex-col border-r border-[var(--color-border)] bg-[var(--wash-row)] p-2.5 sm:p-4">
            <div ref={tabs} role="tablist" aria-label="Settings categories" aria-orientation="vertical" className="min-h-0 space-y-1 overflow-y-auto"
              onKeyDown={(event) => {
                const current = SETTINGS_CATEGORIES.findIndex((entry) => entry.id === category);
                const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
                const target = event.key === "Home" ? 0 : event.key === "End" ? SETTINGS_CATEGORIES.length - 1
                  : step ? (current + step + SETTINGS_CATEGORIES.length) % SETTINGS_CATEGORIES.length : -1;
                const next = SETTINGS_CATEGORIES[target];
                if (!next) return;
                event.preventDefault();
                setCategory(next.id);
                tabs.current?.querySelector<HTMLButtonElement>(`[data-category="${next.id}"]`)?.focus();
              }}>
              {SETTINGS_CATEGORIES.map(({ id: key, label, icon: Icon }) => (
                <button key={key} type="button" role="tab" id={`${id}-tab-${key}`} data-category={key}
                  aria-controls={`${id}-panel-${key}`} aria-selected={category === key} tabIndex={category === key ? 0 : -1}
                  onClick={() => setCategory(key)}
                  className={`flex w-full items-center gap-2.5 rounded-[9px] px-3 py-2.5 text-left text-[13px] transition-colors focus-visible:outline-offset-[-2px] ${category === key ? "bg-[var(--wash-chip)] font-medium text-[var(--color-text-primary)]" : "text-[var(--color-text-secondary)] hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]"}`}>
                  <Icon size={16} className="shrink-0" aria-hidden="true" />{label}
                </button>
              ))}
            </div>
            <p className="mt-auto px-3 pb-1 pt-6 text-[11.5px] leading-[18px] text-[var(--color-text-tertiary)]">
              <LaptopMinimal size={15} className="mb-2" aria-hidden="true" />Threads stay on this device.
            </p>
          </aside>
          <div className="min-h-0 min-w-0">
          <SettingsPanel category="appearance" active={category} idPrefix={id}>
            <div className="flex flex-wrap items-center justify-between gap-4 rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-4 text-[13px]">
              <div>
                <h3 className="font-medium text-[var(--color-text-primary)]">Theme</h3>
                <p className="mt-1 text-[12px] text-[var(--color-text-tertiary)]">Choose a look, or follow your system.</p>
              </div>
              <div
                className="flex gap-1 rounded-[8px] bg-[var(--surface-track)] p-0.5"
                role="radiogroup"
                aria-label="Theme"
                onKeyDown={(event) => {
                  const current = THEME_OPTIONS.findIndex((option) => option.value === theme.preference);
                  const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1
                    : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
                  const target = event.key === "Home" ? 0
                    : event.key === "End" ? THEME_OPTIONS.length - 1
                    : step ? (current + step + THEME_OPTIONS.length) % THEME_OPTIONS.length : -1;
                  const next = target < 0 ? undefined : THEME_OPTIONS[target];
                  if (!next) return;
                  event.preventDefault();
                  theme.setPreference(next.value);
                  (event.currentTarget.children[target] as HTMLElement | undefined)?.focus();
                }}
              >
                {THEME_OPTIONS.map((option) => (
                  <button
                    key={option.value}
                    type="button"
                    role="radio"
                    aria-checked={theme.preference === option.value}
                    tabIndex={theme.preference === option.value ? 0 : -1}
                    onClick={() => theme.setPreference(option.value)}
                    className={[
                      "rounded-[6px] px-2.5 py-1 text-[12.5px] transition-colors",
                      theme.preference === option.value
                        ? "bg-[var(--surface-tile)] text-[var(--color-text-primary)] shadow-glass-tile"
                        : "text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]",
                    ].join(" ")}
                  >
                    {option.label}
                  </button>
                ))}
              </div>
            </div>
          </SettingsPanel>

          <SettingsPanel category="account" active={category} idPrefix={id}>
            <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-4">
            <div className="flex items-center justify-between gap-2">
              {signedIn && <span className="mr-2 flex h-11 w-11 shrink-0 items-center justify-center rounded-full text-[14px] font-medium"
                style={{background: avatar.background, color: avatar.color}} aria-hidden="true">{avatar.initials}</span>}
              <div className="min-w-0">
                <div className="break-words text-[13.5px] font-medium text-[var(--color-text-primary)]">{connected ? accountLabel : "AxiomCLI disconnected"}</div>
                {signedIn && account?.account?.displayName && account.account.verifiedEmail ? <p className="mt-1 break-all text-[12px] text-[var(--color-text-secondary)]">{account.account.verifiedEmail}</p> : null}
                <div className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">
                  {signedIn ? "Signed in" : "Sign in to load models"}
                </div>
                {signedIn && linkedMethods.length > 0 ? (
                  <div className="mt-1 text-[11px] text-[var(--color-text-muted)]">
                    {linkedMethods.join(", ")}
                  </div>
                ) : null}
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
            <div className="mt-4 flex flex-wrap gap-2">
              {signedIn ? (
                <button
                  type="button"
                  onClick={onManageAccount}
                  className="flex items-center gap-1.5 rounded-md border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-[background-color,border-color,color,transform] duration-150 hover:border-[var(--color-border-accent)] hover:bg-[var(--surface-button-secondary-hover)] hover:text-[var(--color-text-primary)] active:scale-[0.97]"
                >
                  <LogIn size={13} />
                  Manage account
                </button>
              ) : null}
              <button
                type="button"
                disabled={!connected}
                onClick={signedIn ? onLogout : onLogin}
                className="flex items-center gap-1.5 rounded-md border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-[background-color,border-color,color,transform] duration-150 hover:border-[var(--color-border-accent)] hover:bg-[var(--surface-button-secondary-hover)] hover:text-[var(--color-text-primary)] active:scale-[0.97] disabled:opacity-50"
              >
                {signedIn ? <LogOut size={13} /> : <LogIn size={13} />}
                {signedIn ? "Sign out" : account?.state === "expired" ? "Sign in again" : "Sign in"}
              </button>
            </div>
            </div>
            <p className="mt-4 text-[12px] leading-5 text-[var(--color-text-tertiary)]">Your sign-in is shared with AxiomCLI on this device.</p>
          </SettingsPanel>


          <SettingsPanel category="usage" active={category} idPrefix={id}>
            {category === "usage" ? <UsagePanel
              key={`${runtimeInstanceId}:${connected}:${account?.state}:${account?.account?.id}`}
              accountId={signedIn ? account?.account?.id ?? null : null}
              connected={connected}
              onLogin={onLogin}
            /> : null}
          </SettingsPanel>
          <SettingsPanel category="api-keys" active={category} idPrefix={id}>
            {category === "api-keys" && (connected && signedIn && account.account ? <ApiKeysPanel
              key={`${runtimeInstanceId}:${account.account.id}`} accountId={account.account.id} onLogin={onLogin}
            /> : <div className="text-[13px] text-[var(--color-text-secondary)]"><p>Sign in to manage API keys.</p><button type="button" disabled={!connected} onClick={onLogin} className="mt-4 rounded-lg border border-[var(--color-border)] px-3 py-2 disabled:opacity-50">Sign in</button></div>)}
          </SettingsPanel>
          <SettingsPanel category="updates" active={category} idPrefix={id}>
            <UpdatesPanel updates={updates} />
          </SettingsPanel>
          </div>
        </div>
    </AccountScreen>
  );
}

import { ArrowUpRight, LogIn, LogOut, RefreshCw, UserRound } from "lucide-react";
import { useId, useState } from "react";
import type { AccountStatus } from "@axiom/axiom-acp-client";
import { useTheme, type ThemePreference } from "../lib/theme";
import { SettingsLayout } from "@axiom/chat-ui/SettingsLayout";
import { SettingsPanel, type SettingsCategory } from "./SettingsPanel";
import { UsagePanel } from "./UsagePanel";
import { McpConnectionsPanel } from "./McpConnectionsPanel";
import { ApiKeysPanel } from "./ApiKeysPanel";
import { UpdatesPanel } from "./UpdatesPanel";
import type { DesktopUpdates } from "../useDesktopUpdates";
import { accountAvatar } from "../lib/accountAvatar";
import { FactoryResetPanel } from "./FactoryResetPanel";
import { accountPresentation } from "../signInFlow";

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
  const theme = useTheme();
  const signedIn = account?.state === "valid";
  const accountView = accountPresentation(connected, true, account);
  const preparingAccount = accountView.kind === "starting";
  const avatar = accountAvatar(account?.account?.displayName);
  const accountLabel = signedIn
    ? (account.account?.displayName || account.account?.verifiedEmail || "Connected Axiom account")
    : accountView.title;
  const linkedMethods = account?.account?.linkedMethods?.map((method) => ({
    passkey: "Passkey",
    google: "Google",
    password: "Email and password",
    ethereum_wallet: "Ethereum wallet",
  })[method]) ?? [];
  return (
    <SettingsLayout category={category} onCategory={setCategory} idPrefix={id} onClose={onClose}>
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
            <div className="max-w-[560px]">
              <div className="overflow-hidden rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)]">
                <div className="flex items-start gap-3.5 p-4">
                  {signedIn ? (
                    <span className="flex h-11 w-11 shrink-0 items-center justify-center rounded-full text-[14px] font-medium"
                      style={{background: avatar.background, color: avatar.color}} aria-hidden="true">{avatar.initials}</span>
                  ) : (
                    <span className="flex h-11 w-11 shrink-0 items-center justify-center rounded-full bg-[var(--wash-chip)] text-[var(--color-text-tertiary)]" aria-hidden="true">
                      <UserRound size={18} />
                    </span>
                  )}
                  <div className="min-w-0 flex-1">
                    <div className="break-words text-[15px] font-medium leading-5 tracking-[-0.01em] text-[var(--color-text-primary)]">{connected ? accountLabel : "AxiomCLI disconnected"}</div>
                    {signedIn && account?.account?.displayName && account.account.verifiedEmail ? <p className="mt-0.5 break-all text-[12.5px] leading-[18px] text-[var(--color-text-secondary)]">{account.account.verifiedEmail}</p> : null}
                    <div className="mt-1.5 flex items-start gap-1.5 text-[12px] leading-[18px] text-[var(--color-text-tertiary)]">
                      {signedIn || account?.state === "expired" || accountView.kind === "unavailable" ? (
                        <span className={`mt-[6px] h-1.5 w-1.5 shrink-0 rounded-full ${signedIn ? "bg-[var(--color-success)]" : "bg-[var(--color-warning)]"}`} aria-hidden="true" />
                      ) : null}
                      <span className="min-w-0 break-words">{signedIn ? "Signed in" : accountView.detail}</span>
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={onRefreshAccount}
                    disabled={!connected || preparingAccount}
                    className="-mr-1 -mt-1 shrink-0 rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)] disabled:opacity-35"
                    aria-label="Refresh account"
                  >
                    <RefreshCw size={14} />
                  </button>
                </div>
                {signedIn && linkedMethods.length > 0 ? (
                  <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-t border-[var(--color-border)] px-4 py-3 text-[12px] leading-[18px]">
                    <span className="text-[var(--color-text-tertiary)]">Sign-in methods</span>
                    <span className="text-[var(--color-text-secondary)]">{linkedMethods.join(", ")}</span>
                  </div>
                ) : null}
                <div className="flex flex-wrap items-center gap-2 border-t border-[var(--color-border)] px-4 py-3">
                  {signedIn ? (
                    <>
                      <button
                        type="button"
                        onClick={onManageAccount}
                        className="flex items-center gap-1.5 rounded-md border border-[var(--color-border-strong)] bg-[var(--surface-button-secondary)] px-3 py-1.5 text-[12.5px] font-medium text-[var(--color-text-primary)] transition-[background-color,border-color,transform] duration-150 hover:border-[var(--color-border-accent)] hover:bg-[var(--surface-button-secondary-hover)] active:scale-[0.97]"
                      >
                        Manage account
                        <ArrowUpRight size={13} />
                      </button>
                      <button
                        type="button"
                        disabled={!connected || preparingAccount}
                        onClick={onLogout}
                        className="ml-auto flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-colors duration-150 enabled:hover:bg-[var(--wash-hover)] enabled:hover:text-[var(--color-text-primary)] disabled:opacity-50"
                      >
                        <LogOut size={13} />
                        Sign out
                      </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      disabled={!connected || preparingAccount}
                      onClick={onLogin}
                      className="flex items-center gap-1.5 rounded-md bg-[var(--color-cherry)] px-3 py-1.5 text-[12.5px] font-medium text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] duration-150 enabled:hover:bg-[var(--color-cherry-bright)] enabled:active:scale-[0.97] disabled:opacity-50"
                    >
                      <LogIn size={13} />
                      {preparingAccount ? "Getting ready…" : account?.state === "expired" ? "Sign in again" : "Sign in"}
                    </button>
                  )}
                </div>
              </div>
              <p className="mt-3 text-[12px] leading-5 text-[var(--color-text-tertiary)]">Your sign-in is shared with AxiomCLI on this device.</p>
              {category === "account" ? <FactoryResetPanel /> : null}
            </div>
          </SettingsPanel>


          <SettingsPanel category="usage" active={category} idPrefix={id}>
            {category === "usage" ? <UsagePanel
              key={`${runtimeInstanceId}:${connected}:${account?.state}:${account?.account?.id}`}
              accountId={signedIn ? account?.account?.id ?? null : null}
              connected={connected}
              onLogin={onLogin}
            /> : null}
          </SettingsPanel>
          <SettingsPanel category="mcp" active={category} idPrefix={id}>
            {category === "mcp" ? connected && signedIn ? <McpConnectionsPanel key={`${runtimeInstanceId}:${account.account?.id}`} /> : <p>Sign in and connect to manage MCP connections.</p> : null}
          </SettingsPanel>
          <SettingsPanel category="api-keys" active={category} idPrefix={id}>
            {category === "api-keys" && (connected && signedIn && account.account ? <ApiKeysPanel
              key={`${runtimeInstanceId}:${account.account.id}`} accountId={account.account.id} onLogin={onLogin}
            /> : <div className="text-[13px] text-[var(--color-text-secondary)]"><p>Sign in to manage API keys.</p><button type="button" disabled={!connected} onClick={onLogin} className="mt-4 rounded-lg border border-[var(--color-border)] px-3 py-2 disabled:opacity-50">Sign in</button></div>)}
          </SettingsPanel>
          <SettingsPanel category="updates" active={category} idPrefix={id}>
            <UpdatesPanel updates={updates} />
          </SettingsPanel>
    </SettingsLayout>
  );
}

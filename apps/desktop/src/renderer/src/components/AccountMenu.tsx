import { ChevronUp, LogIn, LogOut, Settings, WalletCards } from "lucide-react";
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { BillingStatus } from "@axiom/axiom-acp-client";
import type { AccountPresentation } from "../signInFlow";
import { usdFromMicrousd } from "../lib/currency";
import { accountAvatar } from "../lib/accountAvatar";

export interface AccountMenuProps {
  account: AccountPresentation;
  accountId: string | null;
  billing: BillingStatus | null;
  connected: boolean;
  onRefreshBalance: () => Promise<void>;
  onSignIn: () => void;
  onLogout: () => void;
  onOpenSettings: (trigger: HTMLElement) => void;
  onOpenBalance: (trigger: HTMLElement) => void;
}

export function AccountMenu({ account, accountId, billing, connected, onRefreshBalance, onSignIn, onLogout, onOpenSettings, onOpenBalance }: AccountMenuProps) {
  const avatar = accountAvatar(account.kind === "valid" ? account.title : null);
  const [open, setOpen] = useState(false);
  const [refresh, setRefresh] = useState<"loading" | "ready" | "failed">("ready");
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const id = useId();
  const dismiss = useCallback(() => setOpen(false), []);
  const restore = () => { trigger.current?.focus({ preventScroll: true }); dismiss(); };
  useEffect(dismiss, [accountId, account.kind, dismiss]);
  useEffect(() => {
    if (open && trigger.current?.closest("[inert]")) dismiss();
  });
  useEffect(() => {
    if (!open || !connected || account.kind !== "valid") return;
    let cancelled = false;
    setRefresh("loading");
    void onRefreshBalance().then(() => { if (!cancelled) setRefresh("ready"); }, () => { if (!cancelled) setRefresh("failed"); });
    return () => { cancelled = true; };
  }, [open, connected, accountId, account.kind, onRefreshBalance]);

  useLayoutEffect(() => {
    if (!open) return;
    const element = menu.current!;
    const anchor = trigger.current!;
    const rect = anchor.getBoundingClientRect();
    element.style.width = `${Math.min(Math.max(rect.width, 232), innerWidth - 16)}px`;
    const bounds = element.getBoundingClientRect();
    element.style.left = `${Math.max(8, Math.min(rect.left, innerWidth - bounds.width - 8))}px`;
    element.style.top = `${Math.max(8, rect.top - bounds.height - 8)}px`;
    element.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus({ preventScroll: true });
    const outside = (event: Event) => {
      if (!element.contains(event.target as Node) && !anchor.contains(event.target as Node)) dismiss();
    };
    const anchorScrolled = (event: Event) => {
      // Incoming deltas scroll the transcript, not the menu's anchor.
      if (event.target instanceof Node && event.target.contains(anchor)) dismiss();
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("contextmenu", outside, true);
    document.addEventListener("scroll", anchorScrolled, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("blur", dismiss);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("contextmenu", outside, true);
      document.removeEventListener("scroll", anchorScrolled, true);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("blur", dismiss);
    };
  }, [open, dismiss]);

  const balance = account.kind !== "valid" ? "Sign in" : !connected ? "Offline" : refresh === "failed" ? "Unavailable"
    : billing ? usdFromMicrousd(billing.availableMicrousd, 2) : refresh === "loading" ? "Loading…" : "Unavailable";
  const itemClass = "flex w-full items-center gap-2.5 rounded-[7px] px-3 py-2.5 text-left text-[13px] text-[var(--color-text-secondary)] hover:bg-[var(--wash-hover)] focus:bg-[var(--wash-hover)] focus:text-[var(--color-text-primary)] focus:outline-none";
  return <>
    <button ref={trigger} type="button" aria-label="Account menu" aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
      onClick={() => setOpen((value) => !value)}
      onKeyDown={(event) => { if (event.key === "ArrowUp" || event.key === "ArrowDown") { event.preventDefault(); setOpen(true); } }}
      className={`flex w-full min-w-0 items-center gap-2.5 rounded-[10px] px-2 py-2 text-left transition-colors hover:bg-[var(--wash-hover)] ${open ? "bg-[var(--wash-hover)]" : ""}`}>
      <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-[var(--color-cherry)] text-[11px] font-semibold text-on-accent"
        style={account.kind === "valid" ? {background: avatar.background, color: avatar.color} : undefined} aria-hidden="true">
        {account.kind === "valid" ? avatar.initials : <LogIn size={13} strokeWidth={2.3} />}
      </span>
      <span className="flex min-w-0 flex-1 flex-col items-start leading-tight">
        <span className="max-w-full truncate text-[12.5px] text-[var(--color-text-primary)]">{account.title}</span>
        {account.detail ? <span className="max-w-full truncate text-[11px] text-[var(--color-text-tertiary)]">{account.detail}</span> : null}
      </span>
      <ChevronUp size={13} aria-hidden="true" className={`shrink-0 text-[var(--color-text-tertiary)] transition-transform motion-reduce:transition-none ${open ? "rotate-180" : ""}`} />
    </button>
    {open ? createPortal(<div ref={menu} id={id} role="menu" aria-label="Account actions"
      className="account-menu-motion no-drag glass-panel-solid shadow-glass-pop fixed z-[100] max-h-[calc(100dvh-80px)] overflow-y-auto rounded-[12px] p-1.5"
      onKeyDown={(event) => {
        if (event.key === "Escape" || event.key === "Tab") { event.preventDefault(); event.stopPropagation(); restore(); }
        else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          const items = Array.from(menu.current!.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
          const index = items.indexOf(document.activeElement as HTMLButtonElement);
          const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
            : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
          items[next]?.focus();
        }
      }}>
      <button type="button" role="menuitem" className={itemClass} onClick={() => { restore(); onOpenSettings(trigger.current!); }}><Settings size={15} aria-hidden="true" />Settings</button>
      <button type="button" role="menuitem" className={itemClass} onClick={() => { restore(); onOpenBalance(trigger.current!); }}>
        <WalletCards size={15} className="shrink-0" aria-hidden="true" /><span>Balance</span><span className="ml-auto text-[12px] tabular-nums text-[var(--color-text-tertiary)]">{balance}</span>
      </button>
      {account.kind === "valid" ? <>
        <div className="mx-2 my-1 h-px bg-[var(--color-border)]" />
        <button type="button" role="menuitem" className={itemClass} onClick={() => { restore(); onLogout(); }}><LogOut size={15} aria-hidden="true" />Log out</button>
      </> : null}
      {account.kind === "signed-out" || account.kind === "expired" ? <>
        <div className="mx-2 my-1 h-px bg-[var(--color-border)]" />
        <button type="button" role="menuitem" className={itemClass} onClick={() => { restore(); onSignIn(); }}><LogIn size={15} aria-hidden="true" />{account.kind === "expired" ? "Sign in again" : "Sign in"}</button>
      </> : null}
    </div>, document.body) : null}
  </>;
}

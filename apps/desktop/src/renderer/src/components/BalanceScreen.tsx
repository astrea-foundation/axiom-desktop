import { ChevronDown, LogIn, RefreshCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AccountStatus, BillingStatus } from "@axiom/axiom-acp-client";
import { desktopErrorMessage } from "../signInFlow";
import { usdFromMicrousd } from "../lib/currency";
import { DepositPanel } from "./DepositPanel";
import { AccountScreen } from "./AccountScreen";
import { GiftCodePanel } from "./GiftCodePanel";

interface BalanceScreenProps {
  onClose: () => void;
  connected: boolean;
  account: AccountStatus | null;
  billing: BillingStatus | null;
  onRefreshBilling: () => Promise<void>;
  onLogin: () => void;
}

export function BalanceScreen({ onClose, connected, account, billing, onRefreshBilling, onLogin }: BalanceScreenProps) {
  const [billingBusy, setBillingBusy] = useState(false);
  const [billingError, setBillingError] = useState<string | null>(null);
  const content = useRef<HTMLElement>(null);
  const signedIn = account?.state === "valid";
  const shouldPoll = !billing || !!billing.paymentAccount;
  useEffect(() => { content.current?.focus({ preventScroll: true }); }, []);
  useEffect(() => {
    setBillingError(null);
    setBillingBusy(false);
  }, [account?.account?.id, account?.state]);

  useEffect(() => {
    if (!signedIn || !connected) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        await onRefreshBilling();
        if (!stopped) setBillingError(null);
      } catch (error) {
        if (!stopped) setBillingError(shouldPoll
          ? "Deposit status is temporarily unavailable. Retrying…"
          : desktopErrorMessage(error, "Could not load your balance."));
      } finally {
        if (!stopped && shouldPoll) timer = setTimeout(() => void poll(), 5_000);
      }
    };
    void poll();
    return () => { stopped = true; if (timer) clearTimeout(timer); };
  }, [signedIn, connected, account?.account?.id, shouldPoll, onRefreshBilling]);

  const refreshBilling = async () => {
    setBillingBusy(true);
    setBillingError(null);
    try {
      await onRefreshBilling();
    } catch (error) {
      setBillingError(desktopErrorMessage(error, "Could not load your balance."));
    } finally {
      setBillingBusy(false);
    }
  };

  return (
    <AccountScreen title="Balance" onClose={onClose}>
      <section ref={content} tabIndex={-1} aria-label="Balance and payments" className="shadow-glass-card min-h-0 flex-1 overflow-y-auto overscroll-contain rounded-[22px] bg-[var(--surface-card)] p-5 outline-none sm:p-8">
          {signedIn ? (
            <div>
              <h2 className="mb-1 text-[13px] font-medium text-[var(--color-text-secondary)]">Available credit</h2>
              <div className="flex items-start justify-between gap-3">
                <div>
                  <div className="text-[32px] font-semibold tracking-[-0.03em] text-[var(--color-text-primary)]">
                    {billing ? usdFromMicrousd(billing.availableMicrousd, 2) : "—"}
                  </div>
                  {!billing ? <div className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">Loading balance…</div> : null}
                  {billing && billing.postedMicrousd < 0 ? <p role="status" className="mt-2 text-[12px] text-[var(--color-text-secondary)]">Top up to send another message.</p> : null}
                </div>
                <button
                  type="button"
                  onClick={() => void refreshBilling()}
                  disabled={!connected || billingBusy}
                  className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)] disabled:opacity-35"
                  aria-label="Refresh balance"
                >
                  <RefreshCw size={14} className={billingBusy ? "animate-spin" : ""} />
                </button>
              </div>

              {billing ? (
                <details className="group mt-2 text-[12px] text-[var(--color-text-secondary)]">
                  <summary className="inline-flex cursor-pointer list-none items-center gap-1.5 py-1 hover:text-[var(--color-text-primary)]">Credit details <ChevronDown size={12} aria-hidden="true" className="transition-transform group-open:rotate-180" /></summary>
                  <div aria-label="Credit breakdown" className="mt-2 space-y-2 rounded-[10px] bg-[var(--wash-row)] p-3">
                    <div className="flex justify-between gap-3"><span>Available</span><span className="tabular-nums">{usdFromMicrousd(billing.availableMicrousd)}</span></div>
                      <div className="flex justify-between gap-3"><span>Free trial credit</span><span className="tabular-nums">{usdFromMicrousd(billing.trialMicrousd)}</span></div>
                      <div className="flex justify-between gap-3"><span>Other credit</span><span className="tabular-nums">{usdFromMicrousd(billing.paidMicrousd)}</span></div>
                      <p className="text-[var(--color-text-tertiary)]">Trial credit is used first.</p>
                  </div>
                </details>
              ) : null}
              {billing?.paymentReviewRequired ? <p role="alert" className="mt-4 text-[12px] text-[var(--color-warning-strong)]">Spending is paused while a deposit is reviewed. Your trial credit is unchanged.</p> : null}
              {billing?.paymentAccount ? <DepositPanel key={`deposit:${account?.account?.id}`} payment={billing.paymentAccount} connected={connected} quote={billing.zecUsdQuote} /> : billing ? (
                <p className="mt-4 text-[12px] text-[var(--color-text-tertiary)]">Deposits are temporarily unavailable.</p>
              ) : null}

              {account?.account?.id ? <GiftCodePanel key={`gift:${account.account.id}`} accountId={account.account.id} connected={connected} /> : null}

              {billingError ? (
                <p role="alert" className="mt-3 text-[12px] text-[var(--color-danger-strong)]">{billingError}</p>
              ) : null}
            </div>
          ) : (
            <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
              <p className="text-[13px] leading-5 text-[var(--color-text-secondary)]">Sign in to view your balance and payment options.</p>
              <button type="button" disabled={!connected} onClick={onLogin} className="ax-pill ax-pill-button mt-4 disabled:opacity-40"><LogIn size={13} />Sign in</button>
            </div>
          )}

      </section>
    </AccountScreen>
  );
}

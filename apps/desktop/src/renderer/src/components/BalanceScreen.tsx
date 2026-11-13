import { ExternalLink, LogIn, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { AccountStatus, BillingInvoice, BillingStatus } from "@axiom/axiom-acp-client";
import { desktopErrorMessage } from "../signInFlow";
import { usdFromMicrousd } from "../lib/currency";
import { DepositPanel } from "./DepositPanel";
import { AccountScreen } from "./AccountScreen";

interface BalanceScreenProps {
  onClose: () => void;
  connected: boolean;
  account: AccountStatus | null;
  billing: BillingStatus | null;
  invoice: BillingInvoice | null;
  onRefreshBilling: () => Promise<void>;
  onTopUp: (amountUsdCents: number) => Promise<void>;
  onRefreshInvoice: (invoiceId: string) => Promise<void>;
  onLogin: () => void;
}

function usdFromCents(value: number): string {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(value / 100);
}

function parseUsdCents(value: string): number | null {
  const match = /^(0|[1-9][0-9]{0,7})(?:\.([0-9]{1,2}))?$/.exec(value.trim());
  if (!match) return null;
  const dollars = Number(match[1]);
  const cents = Number((match[2] ?? "").padEnd(2, "0"));
  const total = dollars * 100 + cents;
  return Number.isSafeInteger(total) ? total : null;
}

function invoiceStateLabel(state: BillingInvoice["state"]): string {
  return ({
    creating: "Preparing checkout",
    pending: "Waiting for payment",
    paid: "Payment confirmed",
    expired: "Expired",
    failed: "Failed",
    review: "Under review",
  })[state];
}


export function BalanceScreen({ onClose, connected, account, billing, invoice, onRefreshBilling, onTopUp, onRefreshInvoice, onLogin }: BalanceScreenProps) {
  const [topUpAmount, setTopUpAmount] = useState("25.00");
  const [billingBusy, setBillingBusy] = useState(false);
  const [openingCheckout, setOpeningCheckout] = useState(false);
  const [billingError, setBillingError] = useState<string | null>(null);
  const checkoutGeneration = useRef(0);
  const checkoutInFlight = useRef(false);
  const content = useRef<HTMLElement>(null);
  const signedIn = account?.state === "valid";
  const shouldPoll = !billing || !!billing.paymentAccount;
  useEffect(() => { content.current?.focus({ preventScroll: true }); }, []);
  const amountUsdCents = useMemo(() => parseUsdCents(topUpAmount), [topUpAmount]);
  const amountInRange = amountUsdCents !== null
    && billing !== null
    && amountUsdCents >= billing.invoiceMinUsdCents
    && amountUsdCents <= billing.invoiceMaxUsdCents;

  useEffect(() => {
    setBillingError(null);
    setBillingBusy(false);
    setOpeningCheckout(false);
    checkoutInFlight.current = false;
    checkoutGeneration.current += 1;
    return () => { checkoutGeneration.current += 1; };
  }, [account?.account?.id, account?.state]);

  useEffect(() => {
    if (invoice?.state === "paid") setBillingError(null);
  }, [invoice?.state]);

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

  const topUp = async (amount: number) => {
    if (!connected || checkoutInFlight.current) return;
    checkoutInFlight.current = true;
    const generation = ++checkoutGeneration.current;
    setOpeningCheckout(true);
    setBillingError(null);
    try {
      await onTopUp(amount);
    } catch (error) {
      if (generation === checkoutGeneration.current) {
        setBillingError(desktopErrorMessage(error, "Could not open checkout. Please try again."));
      }
    } finally {
      if (generation === checkoutGeneration.current) {
        checkoutInFlight.current = false;
        setOpeningCheckout(false);
      }
    }
  };


  return (
    <AccountScreen title="Balance" onClose={onClose}>
      <section ref={content} tabIndex={-1} aria-label="Balance and payments" className="shadow-glass-card min-h-0 flex-1 overflow-y-auto overscroll-contain rounded-[22px] bg-[var(--surface-card)] p-5 outline-none sm:p-8">
        <p className="mb-6 text-[13px] leading-5 text-[var(--color-text-tertiary)]">Your inference credit and payment options.</p>
          {signedIn ? (
            <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-4 sm:p-5">
              <h3 className="mb-3 text-[13px] font-medium text-[var(--color-text-secondary)]">Inference balance</h3>
              <div className="flex items-start justify-between gap-3">
                <div>
                  <div className="text-[28px] font-semibold tracking-[-0.03em] text-[var(--color-text-primary)]">
                    {billing ? usdFromMicrousd(billing.availableMicrousd) : "—"}
                  </div>
                  <div className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">
                    {billing
                      ? billing.heldMicrousd > 0
                        ? `${usdFromMicrousd(billing.heldMicrousd)} temporarily held`
                        : "Available balance"
                      : "Refresh to load your balance"}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => void refreshBilling()}
                  disabled={!connected || billingBusy || openingCheckout}
                  className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)] disabled:opacity-35"
                  aria-label="Refresh balance"
                >
                  <RefreshCw size={14} className={billingBusy ? "animate-spin" : ""} />
                </button>
              </div>

              {billing?.trialMicrousd != null && billing.paidMicrousd != null ? (
                <div aria-label="Credit breakdown" className="mt-4 space-y-1 text-[12px] text-[var(--color-text-secondary)]">
                  <div className="flex justify-between"><span>Free trial credit</span><span className="tabular-nums">{usdFromMicrousd(billing.trialMicrousd)}</span></div>
                  <div className="flex justify-between"><span>Other inference credit</span><span className="tabular-nums">{usdFromMicrousd(billing.paidMicrousd)}</span></div>
                  <p className="pt-1 text-[var(--color-text-tertiary)]">Trial credit is used first. {billing.paymentAccount?.valuation_enabled ? "Confirmed ZEC deposits add USD-priced inference credit." : "ZEC deposits are tracked separately below."}</p>
                </div>
              ) : null}
              {billing?.paymentReviewRequired ? <p role="alert" className="mt-4 text-[12px] text-[var(--color-cherry)]">A deposit needs review. Inference spending is paused until the payment is reconciled; your trial credit is kept separate.</p> : null}
              {billing?.paymentAccount ? <DepositPanel key={account?.account?.id} payment={billing.paymentAccount} connected={connected} /> : billing?.invoicesEnabled ? (
                <div className="mt-5 border-t border-[var(--color-border)] pt-5">
                  <label htmlFor="top-up-amount" className="text-[12px] font-medium text-[var(--color-text-secondary)]">
                    Top up with Zcash
                  </label>
                  <form className="mt-2 flex gap-2" onSubmit={(event) => {
                    event.preventDefault();
                    if (amountInRange && amountUsdCents !== null) void topUp(amountUsdCents);
                  }}>
                    <div className="flex min-w-0 flex-1 items-center rounded-md border border-[var(--color-border)] bg-[var(--surface-track)] px-3 focus-within:border-[var(--color-border-accent)]">
                      <span className="text-[13px] text-[var(--color-text-tertiary)]">$</span>
                      <input
                        id="top-up-amount"
                        type="text"
                        inputMode="decimal"
                        autoComplete="off"
                        value={topUpAmount}
                        disabled={openingCheckout}
                        onChange={(event) => setTopUpAmount(event.target.value)}
                        className="min-w-0 flex-1 bg-transparent px-1.5 py-2 text-[13.5px] text-[var(--color-text-primary)] outline-none"
                        aria-describedby="top-up-range"
                      />
                    </div>
                    <button
                      type="submit"
                      disabled={!connected || billingBusy || openingCheckout || !amountInRange}
                      aria-busy={openingCheckout}
                      className="rounded-md bg-[var(--color-cherry)] px-3.5 py-2 text-[12.5px] font-medium text-on-accent transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.97] disabled:opacity-40"
                    >
                      {openingCheckout ? "Opening checkout…" : "Top up"}
                    </button>
                  </form>
                  <p id="top-up-range" className="mt-1.5 text-[11px] text-[var(--color-text-muted)]">
                    {`Enter ${usdFromCents(billing.invoiceMinUsdCents)}–${usdFromCents(billing.invoiceMaxUsdCents)}. Shieldz checkout opens in your browser. Your balance updates after payment is confirmed.`}
                  </p>
                </div>
              ) : billing ? (
                <p className="mt-4 text-[12px] text-[var(--color-text-tertiary)]">Deposits are temporarily unavailable.</p>
              ) : null}

              {invoice && !billing?.paymentAccount ? (
                <div className="mt-5 rounded-xl bg-[var(--surface-track)] p-4">
                  <div className="flex items-start justify-between gap-3">
                    <div>
                      <div className="text-[13px] font-medium text-[var(--color-text-primary)]">
                        {usdFromCents(invoice.amountUsdCents)} top-up
                      </div>
                      <div role="status" className="mt-0.5 text-[11.5px] text-[var(--color-text-tertiary)]">
                        {openingCheckout ? "Opening checkout…" : invoiceStateLabel(invoice.state)}
                        {invoice.expiresAt && (invoice.state === "creating" || invoice.state === "pending")
                          ? ` · Expires ${new Date(invoice.expiresAt).toLocaleString()}`
                          : ""}
                      </div>
                    </div>
                    <button
                      type="button"
                      disabled={!connected || openingCheckout}
                      onClick={() => void onRefreshInvoice(invoice.id).catch((error: unknown) => {
                        setBillingError(desktopErrorMessage(error, "Could not refresh this invoice."));
                      })}
                      className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)]"
                      aria-label="Refresh payment status"
                    >
                      <RefreshCw size={13} />
                    </button>
                  </div>
                  {invoice.payUrl && (invoice.state === "creating" || invoice.state === "pending") ? (
                    <button
                      type="button"
                      onClick={() => void topUp(invoice.amountUsdCents)}
                      disabled={!connected || billingBusy || openingCheckout}
                      className="mt-3 flex items-center gap-1.5 rounded-md border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:text-[var(--color-text-primary)] disabled:opacity-40"
                    >
                      Reopen checkout
                      <ExternalLink size={12} />
                    </button>
                  ) : null}
                </div>
              ) : null}

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

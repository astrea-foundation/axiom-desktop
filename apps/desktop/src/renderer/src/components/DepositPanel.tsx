import { Check, ChevronDown, Copy, LoaderCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import type { PaymentAccount, ZecUsdQuote } from "@axiom/axiom-acp-client";
import { usdFromMicrousd } from "../lib/currency";
import { currentZecUsdQuote } from "../zecUsdQuote";

export function zecFromZatoshis(value: string): string {
  if (!/^(0|[1-9][0-9]{0,18})$/.test(value)) throw new Error("Invalid ZEC amount");
  const exact = BigInt(value);
  const decimals = (exact % 100_000_000n).toString().padStart(8, "0").replace(/0+$/, "");
  return `${exact / 100_000_000n}${decimals ? `.${decimals}` : ""}`;
}

export function DepositPanel({ payment, connected, quote }: {
  payment: PaymentAccount;
  connected: boolean;
  quote?: ZecUsdQuote | null;
}) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState("");
  const [now, setNow] = useState(Date.now);
  useEffect(() => { setCopied(false); setCopyError(""); }, [payment.address]);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2_000);
    return () => clearTimeout(timer);
  }, [copied]);
  useEffect(() => {
    const time = Date.now();
    setNow(time);
    const expires = Date.parse(quote?.expires_at ?? "");
    if (!Number.isFinite(expires) || expires <= time) return;
    const timer = setTimeout(() => setNow(Date.now()), Math.min(expires - time, 60_000));
    return () => clearTimeout(timer);
  }, [quote]);
  const currentQuote = connected ? currentZecUsdQuote(quote, now) : null;
  const mainnet = payment.network === "mainnet" && payment.asset === "ZEC" && payment.conversion_status === "none";
  const address = mainnet && payment.state === "ready" && payment.address?.startsWith("u1") ? payment.address : null;
  const uri = address && payment.payment_uri === `zcash:${address}` ? payment.payment_uri : null;
  const copy = async () => {
    if (!address) return;
    try {
      await navigator.clipboard.writeText(address);
      setCopyError("");
      setCopied(true);
    } catch {
      setCopyError("Select the address to copy it.");
    }
  };
  if (!mainnet) return <p role="alert" className="mt-5 text-[13px] text-[var(--color-warning-strong)]">Zcash deposits are unavailable.</p>;
  return <section aria-label="Zcash deposits" className="mt-5 border-t border-[var(--color-border)] pt-5 text-[13px]">
    <header className="mb-5 flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 className="text-[17px] font-medium tracking-[-0.02em]">{payment.valuation_enabled ? "Top up via Zcash" : "Receive ZEC"}</h2>
      </div>
      {payment.valuation_enabled ? <div aria-label="Current ZEC exchange rate" className="text-right">
        <p className="font-medium tabular-nums">{currentQuote
          ? `1 ZEC ≈ ${usdFromMicrousd(Number(currentQuote.price_microusd_per_zec), 2)}`
          : "Rate unavailable"}</p>
        <p className="mt-1 text-[11.5px] text-[var(--color-text-tertiary)]">Rate at confirmation applies</p>
      </div> : null}
    </header>
    {payment.review_required ? <p role="alert" className="mb-4 text-[var(--color-warning-strong)]">A deposit is under review.</p> : null}
    {!connected ? <p role="status" className="mb-4 text-[var(--color-text-secondary)]">Deposit updates paused while offline</p> : null}
    {address && uri ? <div className="grid items-center gap-5 min-[600px]:grid-cols-[192px_minmax(0,1fr)] min-[600px]:gap-6">
      <div className="justify-self-center overflow-hidden rounded-[12px]">
        <QRCodeSVG value={uri} size={192} marginSize={4} level="M" title="Mainnet Zcash deposit payment URI" />
      </div>
      <div className="min-w-0">
        <p className="mb-2 text-[12px] text-[var(--color-text-secondary)]">Reusable address</p>
        <div aria-label="Mainnet deposit address" tabIndex={0}
          className="selectable break-all rounded-[10px] bg-[var(--surface-track)] p-3 font-mono text-[12px] leading-5">{address}</div>
        <button type="button" onClick={() => void copy()} aria-label={copied ? "Copied" : "Copy address"}
          className="mt-3 inline-flex h-9 items-center justify-center gap-2 rounded-[8px] bg-[var(--color-cherry)] px-4 text-[12.5px] font-medium text-on-accent transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.97]">
          {copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
          <span role="status">{copied ? "Copied" : "Copy address"}</span>
        </button>
        {copyError ? <p role="status" className="mt-2 text-[12px] text-[var(--color-text-secondary)]">{copyError}</p> : null}
      </div>
    </div> : <p role="status" className="py-6 text-[var(--color-text-secondary)]">Preparing your address…</p>}
    <div className="mt-5 border-t border-[var(--color-border)] pt-4">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-[12px] font-medium text-[var(--color-text-secondary)]">Recent deposits</h3>
        {!payment.deposits.length ? <span className="text-[12px] text-[var(--color-text-tertiary)]">No deposits yet</span> : null}
      </div>
      {payment.deposits.length ? <ul className="mt-2 divide-y divide-[var(--color-border)]">
        {payment.deposits.map((deposit) => {
          const valued = deposit.credit_microusd != null && (deposit.valuation_status === "credited" || deposit.valuation_status === "reversed");
          const status = deposit.review_required || deposit.valuation_status === "review" ? "Under review"
            : deposit.state === "orphaned" || deposit.valuation_status === "reversed" ? "Reversed"
            : payment.valuation_enabled && !valued && deposit.state === "confirmed" ? "Calculating credit"
            : deposit.state === "confirming" ? "Confirming" : "Confirmed";
          const estimate = payment.valuation_enabled && currentQuote && (status === "Confirming" || status === "Calculating credit")
            ? BigInt(deposit.amount_zatoshis) * BigInt(currentQuote.price_microusd_per_zec) / 100_000_000n : null;
          const usdAmount = valued ? usdFromMicrousd(Number(deposit.credit_microusd), 2)
            : estimate !== null ? `~${usdFromMicrousd(Number(estimate), 2)}` : null;
          const confirmations = BigInt(deposit.confirmations) > BigInt(deposit.required_confirmations)
            ? deposit.required_confirmations : deposit.confirmations;
          return <li key={deposit.id}>
            <details className="group py-3">
              <summary className="flex cursor-pointer list-none items-center justify-between gap-3">
                <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
                  <span className="inline-flex items-baseline gap-2 whitespace-nowrap tabular-nums">
                    <span className="font-medium">{zecFromZatoshis(deposit.amount_zatoshis)} ZEC</span>
                    {usdAmount !== null ? <span className="text-[12px] text-[var(--color-text-tertiary)]">{usdAmount}</span> : null}
                  </span>
                  <span role="status" className="inline-flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[12px] text-[var(--color-text-tertiary)]">
                    {status === "Confirming" || status === "Calculating credit"
                      ? <LoaderCircle size={13} aria-hidden="true" className="shrink-0 animate-spin motion-reduce:animate-none" />
                      : status === "Confirmed" ? <Check size={13} aria-hidden="true" className="shrink-0 text-[var(--color-success)]" /> : null}
                    <span>{status}</span>
                    <span className="tabular-nums">{confirmations}/{deposit.required_confirmations} confirmations</span>
                  </span>
                </div>
                <span className="flex shrink-0 items-center gap-2 text-[11.5px] text-[var(--color-text-tertiary)]">
                  {new Date(deposit.observed_at).toLocaleDateString(undefined, { month: "short", day: "numeric" })}
                  <ChevronDown size={12} aria-hidden="true" className="transition-transform group-open:rotate-180" />
                </span>
              </summary>
              <div className="mt-2 space-y-1 text-[12px] leading-5 text-[var(--color-text-secondary)]">
                <p className="flex flex-wrap gap-x-3"><span>{zecFromZatoshis(deposit.amount_zatoshis)} ZEC</span><span>{deposit.confirmations}/{deposit.required_confirmations} confirmations</span></p>
                {valued ? <p className="flex flex-wrap gap-x-3"><span>{usdFromMicrousd(Number(deposit.credit_microusd), 2)} credit {deposit.valuation_status === "credited" ? "added" : "reversed"}</span>
                  {deposit.price_microusd_per_zec && deposit.price_source ? <span>{usdFromMicrousd(Number(deposit.price_microusd_per_zec), 2)}/ZEC</span> : null}</p> : null}
                {status === "Calculating credit" ? <p>Payment received. Credit calculation will retry automatically.</p> : null}
                <p className="text-[var(--color-text-tertiary)]">{new Date(deposit.observed_at).toLocaleString()}</p>
              </div>
            </details>
          </li>;
        })}
      </ul> : null}
    </div>
  </section>;
}

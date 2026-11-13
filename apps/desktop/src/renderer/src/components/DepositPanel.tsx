import { useEffect, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import type { PaymentAccount } from "@axiom/axiom-acp-client";
import { usdFromMicrousd } from "../lib/currency";

export function zecFromZatoshis(value: string): string {
  if (!/^(0|[1-9][0-9]{0,18})$/.test(value)) throw new Error("Invalid ZEC amount");
  const exact = BigInt(value);
  const decimals = (exact % 100_000_000n).toString().padStart(8, "0").replace(/0+$/, "");
  return `${exact / 100_000_000n}${decimals ? `.${decimals}` : ""}`;
}

export function DepositPanel({ payment, connected }: { payment: PaymentAccount; connected: boolean }) {
  const [copyStatus, setCopyStatus] = useState("");
  useEffect(() => { setCopyStatus(""); }, [payment.address]);
  const mainnet = payment.network === "mainnet" && payment.asset === "ZEC" && payment.conversion_status === "none";
  const address = mainnet && payment.state === "ready" && payment.address?.startsWith("u1") ? payment.address : null;
  const uri = address && payment.payment_uri === `zcash:${address}` ? payment.payment_uri : null;
  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus("Copied");
    } catch {
      setCopyStatus("Copy unavailable. Select and copy the address below.");
    }
  };
  if (!mainnet) return <p role="alert" className="mt-5 text-[13px] text-[var(--color-warning-strong)]">Mainnet payments are not configured yet. No payment address is available.</p>;
  return <section aria-label="Zcash deposits" className="mt-5 space-y-4 border-t border-[var(--color-border)] pt-5 text-[13px]">
    <h3 className="font-semibold">{payment.valuation_enabled ? "Top up with Zcash" : "Receive ZEC"}</h3>
    <p>{payment.valuation_enabled
      ? "Send ZEC on Zcash mainnet. Once confirmed, its USD value is added to your inference credit at the confirmation-time rate. The funds remain in ZEC; no USDC conversion is made."
      : "Send ZEC on Zcash mainnet only. Deposits stay in ZEC and are tracked separately from inference credit."}</p>
    {payment.review_required ? <p role="alert" className="text-[var(--color-cherry)]">Deposit history needs operator review. Confirmation can be revoked and later restored.</p> : null}
    {!connected ? <p role="status">Disconnected. Deposit status may be out of date.</p> : null}
    <div className="flex flex-wrap gap-x-6 gap-y-2">
      <span>Confirmed: <strong>{zecFromZatoshis(payment.confirmed_zatoshis)} ZEC</strong></span>
      <span>Confirming: {zecFromZatoshis(payment.confirming_zatoshis)} ZEC</span>
    </div>
    {address && uri ? <>
      <div className="flex justify-center">
        <QRCodeSVG value={uri} size={224} marginSize={4} level="M" title="Mainnet Zcash deposit payment URI" />
      </div>
      <label className="block">Your reusable Zcash deposit address
        <textarea readOnly value={address} aria-label="Mainnet deposit address" rows={4}
          className="mt-2 block w-full resize-none break-all rounded-md border border-[var(--color-border)] bg-[var(--surface-track)] p-3 font-mono text-[12px]" />
      </label>
      <div className="flex flex-wrap gap-3">
        <button className="ax-pill ax-pill-button" onClick={() => void copy(address)}>Copy address</button>
        <button className="ax-pill ax-pill-button" onClick={() => void copy(uri)}>Copy payment URI</button>
      </div>
      <p className="break-all font-mono text-[11px]" aria-label="Payment URI">{uri}</p>
      <p>Send any amount, repeatedly, to this address. No invoice, memo, or expiration. Required confirmations: {payment.required_confirmations ?? "pending"}. Confirmations are not absolute finality.</p>
      <p className="text-[var(--color-text-tertiary)]">Receiver status: {payment.monitoring_status}</p>
    </> : <p role="status">Preparing your persistent address… This view checks again automatically.</p>}
    {copyStatus ? <p role="status">{copyStatus}</p> : null}
    {payment.deposits.length ? <div>
      <h4 className="mb-2 font-medium">Recent deposits</h4>
      <ul className="space-y-2">
        {payment.deposits.map((deposit) => <li key={deposit.id} className="rounded-md bg-[var(--surface-track)] p-3">
          <span>{zecFromZatoshis(deposit.amount_zatoshis)} ZEC · {deposit.state === "orphaned" ? "Reversed (chain reorganization)" : deposit.state}</span>
          <div className="text-[11px] text-[var(--color-text-tertiary)]">{deposit.confirmations}/{deposit.required_confirmations} confirmations · {new Date(deposit.observed_at).toLocaleString()}</div>
          {payment.valuation_enabled && deposit.valuation_status === "pending" ? <p role="status" className="mt-1 text-[12px]">Payment received—credit calculation pending. We’ll retry automatically.</p> : null}
          {deposit.credit_microusd != null && (deposit.valuation_status === "credited" || deposit.valuation_status === "reversed") ? <p className="mt-1 text-[12px]">
            {usdFromMicrousd(Number(deposit.credit_microusd))} inference credit {deposit.valuation_status === "credited" ? "added" : "reversed"}
            {deposit.price_microusd_per_zec && deposit.price_source ? <span className="text-[var(--color-text-tertiary)]"> · {usdFromMicrousd(Number(deposit.price_microusd_per_zec))}/ZEC ({deposit.price_source === "coinbase" ? "Coinbase" : "Kraken"})</span> : null}
          </p> : null}
          {deposit.valuation_status === "review" && !deposit.review_required ? <p className="mt-1 text-[12px]">Credit under review</p> : null}
          {deposit.review_required ? <span>Under review</span> : null}
        </li>)}
      </ul>
    </div> : <p>No deposits received yet.</p>}
  </section>;
}

import { useEffect, useRef, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import type { CryptoOptions, CryptoPayment, CreateCryptoPaymentRequest, PaymentAccount, ZecUsdQuote } from "@axiom/axiom-acp-client";
import { desktopErrorMessage } from "../signInFlow";
import { DepositPanel } from "./DepositPanel";
import { usdFromMicrousd } from "../lib/currency";

export function creditAmount(value: string): number | null {
  if (!/^\d{1,4}(?:\.\d{1,2})?$/.test(value)) return null;
  const [whole, fraction = ""] = value.split(".");
  const amount = Number(whole) * 1_000_000 + Number(fraction.padEnd(2, "0")) * 10_000;
  return amount >= 5_000_000 && amount <= 1_000_000_000 ? amount : null;
}

export function cryptoStatus(payment: CryptoPayment, now: number): string {
  if (payment.review_required || payment.status === "review") return "Under review";
  if (payment.status === "refunded") return "Refunded";
  if (payment.credited_microusd > 0) return "Credited";
  if (payment.status === "waiting" && Date.parse(payment.expires_at ?? "") <= now) return "Expired";
  return ({creating: "Preparing payment", waiting: "Awaiting payment", confirming: "Confirming", confirmed: "Confirming",
    sending: "Confirming", partially_paid: "Partial payment", finished: "Confirming", failed: "Failed", expired: "Expired", refunded: "Refunded"} as Record<string, string>)[payment.status] ?? "Under review";
}

export function CryptoPaymentDetails({ payment, options, now = Date.now() }: { payment: CryptoPayment; options: CryptoOptions; now?: number }) {
  const [copied, setCopied] = useState<string | null>(null);
  const [copyError, setCopyError] = useState(false);
  const generation = useRef(0);
  useEffect(() => { generation.current++; setCopied(null); setCopyError(false); return () => { generation.current++; }; }, [payment.id]);
  const coin = options.currencies.find((item) => item.code === payment.pay_currency);
  const status = cryptoStatus(payment, now);
  const payable = status === "Awaiting payment" && !!coin && !!payment.pay_address && !!payment.pay_amount
    && Date.parse(payment.expires_at ?? "") > now;
  const copy = async (value: string, label: string) => {
    const current = generation.current;
    try { await navigator.clipboard.writeText(value); if (generation.current === current) { setCopied(label); setCopyError(false); } }
    catch { if (generation.current === current) setCopyError(true); }
  };
  return <div className="mt-3 space-y-3 rounded-xl bg-[var(--wash-row)] p-4 text-[13px]">
    <div className="flex justify-between gap-3"><span>{usdFromMicrousd(payment.amount_microusd, 2)} credit</span><span role="status">{status}</span></div>
    <p className="text-[12px] text-[var(--color-text-secondary)]">{coin?.name ?? payment.pay_currency.toUpperCase()}{coin ? ` · ${coin.network}` : ""}</p>
    {payable ? <>
      <p className="selectable break-all font-mono">{payment.pay_amount} {payment.pay_currency.toUpperCase()}</p>
      <button type="button" onClick={() => void copy(payment.pay_amount!, "amount")} className="ax-pill ax-pill-button">{copied === "amount" ? "Copied" : "Copy amount"}</button>
      <div className="flex flex-col items-start gap-3 sm:flex-row">
        <QRCodeSVG value={payment.pay_address!} size={160} marginSize={4} title="Crypto payment address" role="img" aria-label="Crypto payment address" />
        <div className="min-w-0 flex-1"><p className="selectable break-all font-mono text-[12px]" aria-label="Crypto address">{payment.pay_address}</p>
          <button type="button" onClick={() => void copy(payment.pay_address!, "address")} className="ax-pill ax-pill-button mt-3">{copied === "address" ? "Copied" : "Copy address"}</button>
        </div>
      </div>
      {payment.payin_extra_id ? <div><p className="selectable break-all font-mono">Memo: {payment.payin_extra_id}</p>
        <button type="button" onClick={() => void copy(payment.payin_extra_id!, "memo")} className="ax-pill ax-pill-button mt-2">{copied === "memo" ? "Copied" : "Copy memo"}</button></div> : null}
      <p className="text-[12px] text-[var(--color-text-secondary)]">Use {coin!.network}. Pay once before {new Date(payment.expires_at!).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})}.</p>
    </> : null}
    {status === "Partial payment" ? <p className="text-[12px] text-[var(--color-text-secondary)]">Payment needs review.</p> : null}
    {copyError ? <p role="alert">Select the value to copy it.</p> : null}
  </div>;
}

export function FundingPanel({ accountId, connected, payment, quote, onRefreshBilling }: {
  accountId: string; connected: boolean; payment?: PaymentAccount | null; quote?: ZecUsdQuote | null; onRefreshBilling: () => Promise<void>;
}) {
  const [options, setOptions] = useState<CryptoOptions | null>(null);
  const [open, setOpen] = useState(false);
  const [coin, setCoin] = useState("");
  const [amount, setAmount] = useState("25");
  const [payments, setPayments] = useState<CryptoPayment[]>([]);
  const [selected, setSelected] = useState<CryptoPayment | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now);
  const generation = useRef(0);
  const intent = useRef<CreateCryptoPaymentRequest | null>(null);
  const pending = useRef(false);
  const credited = useRef(new Set<string>());
  useEffect(() => {
    generation.current++;
    setOptions(null); setOpen(false); setPayments([]); setSelected(null); setError(null);
    intent.current = null; credited.current.clear();
  }, [accountId]);
  useEffect(() => {
    const current = ++generation.current;
    setBusy(false); pending.current = false;
    const api = window.axiomDesktop?.agent;
    if (connected && api?.cryptoOptions) void api.cryptoOptions(accountId).then(({options: value}) => {
      if (current !== generation.current) return;
      setOptions(value); setCoin((previous) => value.currencies.some((c) => c.code === previous) ? previous : value.currencies[0]?.code ?? "");
    }).catch(() => {});
    return () => { generation.current++; };
  }, [accountId, connected]);

  useEffect(() => {
    if (!open || !connected || !options?.enabled) return;
    const current = generation.current;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const api = window.axiomDesktop?.agent;
        if (!api) return;
        const result = await api.cryptoPayments(accountId);
        if (stopped || generation.current !== current) return;
        setPayments(result.payments);
        setSelected((previous) => previous ? result.payments.find((p) => p.id === previous.id) ?? previous : null);
        setError((previous) => previous === "Status unavailable. Retrying…" ? null : previous);
        const newlyCredited = result.payments.filter((p) => p.credited_microusd > 0 && !credited.current.has(p.id));
        newlyCredited.forEach((p) => credited.current.add(p.id));
        if (newlyCredited.length) await onRefreshBilling();
      } catch { if (!stopped && generation.current === current) setError("Status unavailable. Retrying…"); }
      finally { if (!stopped && generation.current === current) { setNow(Date.now()); timer = setTimeout(() => void poll(), 5_000); } }
    };
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [open, connected, accountId, options?.enabled, onRefreshBilling]);

  useEffect(() => {
    if (!selected) return;
    const timer = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(timer);
  }, [selected?.id]);

  const create = async () => {
    const api = window.axiomDesktop?.agent;
    const value = creditAmount(amount);
    if (!api || !connected || !value || !coin || pending.current) return;
    intent.current ??= {id: crypto.randomUUID(), amountMicrousd: value, payCurrency: coin};
    const current = generation.current;
    pending.current = true; setBusy(true); setError(null);
    try {
      const result = await api.createCryptoPayment(intent.current, accountId);
      if (generation.current !== current) return;
      setSelected(result.payment); intent.current = null;
      setPayments((previous) => [result.payment, ...previous.filter((p) => p.id !== result.payment.id)]);
    } catch (failure) { if (generation.current === current) {
      const message = desktopErrorMessage(failure, "Could not confirm payment. Retry safely.");
      setError(message);
      if (message.includes("Increase the amount")) intent.current = null;
    } }
    finally { if (generation.current === current) { pending.current = false; setBusy(false); } }
  };
  const fieldClass = "mt-1 block w-full rounded-lg border border-[var(--color-border)] bg-[var(--wash-row)] px-3 py-2 text-[13px] outline-none disabled:opacity-40";
  return <>
    {payment ? <DepositPanel payment={payment} connected={connected} quote={quote} discountBps={options?.zcash_discount_bps ?? 0} /> : payment === null ? <p className="mt-4 text-[12px] text-[var(--color-text-tertiary)]">Deposits are temporarily unavailable.</p> : null}
    {options?.enabled && options.currencies.length ? <section className="mt-5 border-t border-[var(--color-border)] pt-5 text-[13px]" aria-label="Other crypto">
      <button type="button" className="ax-pill ax-pill-button" aria-expanded={open} onClick={() => setOpen(!open)}>Other crypto</button>
      {open ? <>
        {selected ? <><CryptoPaymentDetails payment={selected} options={options} now={now} />
          <button type="button" className="ax-pill ax-pill-button mt-3" disabled={!connected || busy} onClick={() => { setSelected(null); intent.current = null; setError(null); }}>New payment</button></> :
          <form className="mt-3 flex flex-wrap items-end gap-3" onSubmit={(e) => { e.preventDefault(); void create(); }}>
            <label className="min-w-0 flex-1 text-[12px]">Crypto<select aria-label="Crypto" value={coin} disabled={!connected || busy || !!intent.current} onChange={(e) => { intent.current = null; setCoin(e.target.value); setError(null); }} className={fieldClass}>
              {options.currencies.map((c) => <option key={c.code} value={c.code}>{c.name} · {c.network}</option>)}
            </select></label>
            <label className="text-[12px]">Amount (USD credit)<input aria-label="Amount (USD credit)" className={`${fieldClass} w-32`} inputMode="decimal" value={amount} disabled={!connected || busy || !!intent.current} maxLength={7} onChange={(e) => { intent.current = null; setAmount(e.target.value); setError(null); }} /></label>
            <button type="submit" className="ax-pill ax-pill-button disabled:opacity-40" disabled={!connected || busy || creditAmount(amount) === null}>{busy ? "Creating…" : intent.current ? "Retry" : "Create payment"}</button>
            {creditAmount(amount) === null ? <p className="w-full text-[12px]">Enter $5–$1,000.</p> : null}
          </form>}
        {payments.length ? <details className="mt-3"><summary className="cursor-pointer text-[12px]">Recent payments</summary>
          <ul className="mt-2 space-y-2">{payments.map((p) => <li key={p.id}><button type="button" className="text-left text-[12px]" onClick={() => { setSelected(p); intent.current = null; setError(null); }}>
            {usdFromMicrousd(p.amount_microusd, 2)} · {p.pay_currency.toUpperCase()} · {cryptoStatus(p, now)}
          </button></li>)}</ul>
        </details> : null}
        {error ? <p role="alert" className="mt-3 text-[12px] text-[var(--color-danger-strong)]">{error}</p> : null}
        {!connected ? <p role="status" className="mt-3 text-[12px]">Reconnecting…</p> : null}
      </> : null}
    </section> : null}
  </>;
}

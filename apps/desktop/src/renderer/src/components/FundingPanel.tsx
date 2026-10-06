import { useEffect, useRef, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import { ArrowRight, ChevronRight, Copy, Plus, TriangleAlert } from "lucide-react";
import type { CryptoCurrency, CryptoOptions, CryptoPayment, CreateCryptoPaymentRequest, PaymentAccount, ZecUsdQuote } from "@axiom/axiom-acp-client";
import { desktopErrorMessage } from "../signInFlow";
import { DepositPanel } from "./DepositPanel";
import { usdFromMicrousd } from "../lib/currency";
import { CryptoIcon, cryptoName, cryptoNetwork, cryptoSymbol } from "./CryptoIcon";
import { CryptoCurrencyPicker } from "./CryptoCurrencyPicker";
import { CryptoDepositDialog } from "./CryptoDepositDialog";

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

function CryptoNetworkWarning({ coin, currencies }: { coin?: CryptoCurrency; currencies: CryptoCurrency[] }) {
  if (!coin) return null;
  const multipleNetworks = coin.code === "eth" || coin.code.startsWith("usdc") || coin.code.startsWith("usdt")
    || currencies.some((other) => other.code !== coin.code && cryptoName(other) === cryptoName(coin) && other.network !== coin.network);
  if (!multipleNetworks) return null;
  return <div role="alert" className="flex items-start gap-2.5 rounded-xl border border-[var(--color-warning-border)] bg-[var(--color-warning-soft)] p-3 text-[11px] leading-5 text-[var(--color-warning-strong)]">
    <TriangleAlert size={15} aria-hidden="true" className="mt-0.5 shrink-0" />
    <div><p className="font-medium">Network: {cryptoNetwork(coin)} only.</p><p>Funds sent on other networks may be permanently lost.</p></div>
  </div>;
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
  const copyClass = "rounded-lg p-2 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline-2 focus-visible:outline-offset-2";
  return <div className="space-y-5 text-[13px]">
    <div className="flex items-center gap-3">
      <CryptoIcon code={payment.pay_currency} className="h-9 w-9" />
      <div className="min-w-0 flex-1"><p className="font-medium">{coin ? cryptoName(coin) : cryptoSymbol(payment.pay_currency)}</p>
        {coin && cryptoNetwork(coin) !== cryptoName(coin) ? <p className="mt-0.5 text-[11px] text-[var(--color-text-tertiary)]">{cryptoNetwork(coin)}</p> : null}</div>
      <span role="status" className={`rounded-full px-2.5 py-1 text-[11px] ${status === "Credited" ? "bg-[var(--color-success-glow)] text-[var(--color-success)]" : "bg-[var(--wash-chip)] text-[var(--color-text-secondary)]"}`}>{status}</span>
    </div>
    {payable ? <>
      <CryptoNetworkWarning coin={coin} currencies={options.currencies} />
      <div className="rounded-2xl bg-[var(--wash-row)] px-4 py-5">
        <div className="mb-5 flex items-center justify-center gap-1.5">
          <p className="selectable min-w-0 break-all text-[19px] font-medium tracking-tight tabular-nums">{payment.pay_amount} <span className="text-[13px] text-[var(--color-text-secondary)]">{cryptoSymbol(payment.pay_currency)}</span></p>
          <button type="button" aria-label={copied === "amount" ? "Amount copied" : "Copy amount"} onClick={() => void copy(payment.pay_amount!, "amount")} className={copyClass}><Copy size={14} aria-hidden="true" /></button>
        </div>
        <div className="mx-auto w-fit overflow-hidden rounded-xl bg-white p-2"><QRCodeSVG value={payment.pay_address!} size={168} marginSize={2} title="Crypto payment address" role="img" aria-label="Crypto payment address" /></div>
        <div className="mb-1 mt-4 flex items-center justify-between gap-3"><span className="text-[11px] text-[var(--color-text-tertiary)]">Deposit address</span>
          <button type="button" aria-label={copied === "address" ? "Address copied" : "Copy address"} onClick={() => void copy(payment.pay_address!, "address")} className={copyClass}><Copy size={14} aria-hidden="true" /></button></div>
        <p className="selectable break-all font-mono text-[11px] leading-5" aria-label="Crypto address">{payment.pay_address}</p>
      </div>
      {payment.payin_extra_id ? <div className="rounded-xl border border-[var(--color-border)] p-3"><p className="selectable break-all font-mono text-[12px]">Memo: {payment.payin_extra_id}</p>
        <button type="button" onClick={() => void copy(payment.payin_extra_id!, "memo")} className="ax-pill ax-pill-button mt-2">{copied === "memo" ? "Copied" : "Copy memo"}</button></div> : null}
      <p className="text-[11px] leading-5 text-[var(--color-text-secondary)]">Use {cryptoNetwork(coin!)}. Pay once before {new Date(payment.expires_at!).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})}.</p>
    </> : null}
    <div className="flex justify-between gap-3 border-t border-[var(--color-border)] pt-4"><span className="text-[var(--color-text-secondary)]">USD credit</span><span className="font-medium tabular-nums">{usdFromMicrousd(payment.amount_microusd, 2)}</span></div>
    {status === "Partial payment" ? <p className="text-[12px] text-[var(--color-text-secondary)]">Payment needs review.</p> : null}
    {copied ? <p role="status" className="sr-only">Copied {copied}</p> : null}
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
      setOptions(value); setCoin((previous) => value.currencies.some((c) => c.code === previous) ? previous : "");
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
  return <>
    {payment ? <DepositPanel payment={payment} connected={connected} quote={quote} discountBps={options?.zcash_discount_bps ?? 0} /> : payment === null ? <p className="mt-4 text-[12px] text-[var(--color-text-tertiary)]">Deposits are temporarily unavailable.</p> : null}
    {options?.enabled && options.currencies.length ? <section className="mt-5 border-t border-[var(--color-border)] pt-5 text-[13px]" aria-label="Other crypto">
      <button type="button" aria-label="Other crypto" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}
        className="group flex w-full items-center justify-between gap-4 rounded-2xl border border-[var(--color-border)] bg-[var(--surface-tile)] px-4 py-4 text-left transition-[border-color,background-color,transform] hover:border-[var(--color-border-strong)] hover:bg-[var(--surface-card-hover)] active:scale-[0.99] focus-visible:outline-2 focus-visible:outline-offset-2">
        <span className="font-medium">Other crypto</span>
        <span aria-hidden="true" className="flex items-center gap-3"><span className="flex -space-x-1.5">
          {["btc", "usdc", "xmr"].filter((code) => options.currencies.some((c) => c.code === code)).map((code) => <span key={code} className="rounded-full ring-2 ring-[var(--surface-tile)]"><CryptoIcon code={code} className="h-7 w-7" /></span>)}
          <span className="inline-flex h-7 w-7 items-center justify-center rounded-full bg-[var(--color-bg-surface-hover)] text-[var(--color-text-secondary)] ring-2 ring-[var(--surface-tile)]"><Plus size={13} /></span>
        </span><ChevronRight size={16} className="text-[var(--color-text-tertiary)] transition-transform group-hover:translate-x-0.5" /></span>
      </button>
      {open ? <CryptoDepositDialog onClose={() => setOpen(false)}>
        {selected ? <><CryptoPaymentDetails payment={selected} options={options} now={now} />
          <button type="button" className="ax-pill ax-pill-button mt-5 w-full justify-center" disabled={!connected || busy} onClick={() => { setSelected(null); intent.current = null; setError(null); }}>New payment</button></> :
          <form className="space-y-5" onSubmit={(e) => { e.preventDefault(); void create(); }}>
            <div><CryptoCurrencyPicker currencies={options.currencies} value={coin} disabled={!connected || busy || !!intent.current}
              onChange={(value) => { intent.current = null; setCoin(value); setError(null); }} />
            </div>
            <CryptoNetworkWarning coin={options.currencies.find((c) => c.code === coin)} currencies={options.currencies} />
            <div><label htmlFor={`crypto-amount-${accountId}`} className="mb-2 block text-[12px] font-medium text-[var(--color-text-secondary)]">Amount (USD credit)</label>
              <div className="flex min-h-14 items-center gap-2 rounded-xl border border-[var(--color-border)] bg-[var(--wash-row)] px-3.5 transition-colors focus-within:border-[var(--color-border-accent)]">
                <span aria-hidden="true" className="text-[16px] text-[var(--color-text-tertiary)]">$</span>
                <input id={`crypto-amount-${accountId}`} className="min-w-0 flex-1 bg-transparent py-3 text-[18px] font-medium tabular-nums outline-none disabled:opacity-40" inputMode="decimal" value={amount} disabled={!connected || busy || !!intent.current} maxLength={7}
                  aria-invalid={creditAmount(amount) === null} onChange={(e) => { intent.current = null; setAmount(e.target.value); setError(null); }} />
                <span aria-hidden="true" className="text-[11px] text-[var(--color-text-tertiary)]">USD</span>
              </div>
              {creditAmount(amount) === null ? <p className="mt-2 text-[12px] text-[var(--color-danger-strong)]">Enter $5–$1,000.</p> : null}
            </div>
            <button type="submit" className="flex min-h-12 w-full items-center justify-center gap-2 rounded-xl bg-[var(--color-cherry)] px-4 py-3 text-[13px] font-medium text-on-accent transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.98] disabled:opacity-35" disabled={!connected || busy || !coin || creditAmount(amount) === null}>
              {busy ? "Creating…" : intent.current ? "Retry" : "Create payment"}<ArrowRight size={15} aria-hidden="true" />
            </button>
          </form>}
        {payments.length ? <details className="mt-5 border-t border-[var(--color-border)] pt-4"><summary className="cursor-pointer text-[12px] text-[var(--color-text-secondary)]">Recent payments</summary>
          <ul className="mt-3 space-y-1">{payments.map((p) => <li key={p.id}><button type="button" className="flex w-full items-center gap-2.5 rounded-lg p-2 text-left text-[12px] hover:bg-[var(--wash-row)]" onClick={() => { setSelected(p); intent.current = null; setError(null); }}>
            <CryptoIcon code={p.pay_currency} className="h-5 w-5" /><span className="flex-1 tabular-nums">{usdFromMicrousd(p.amount_microusd, 2)} · {cryptoSymbol(p.pay_currency)}</span><span className="text-[11px] text-[var(--color-text-tertiary)]">{cryptoStatus(p, now)}</span>
          </button></li>)}</ul>
        </details> : null}
        {error ? <p role="alert" className="mt-3 text-[12px] text-[var(--color-danger-strong)]">{error}</p> : null}
        {!connected ? <p role="status" className="mt-3 text-[12px]">Reconnecting…</p> : null}
      </CryptoDepositDialog> : null}
    </section> : null}
  </>;
}

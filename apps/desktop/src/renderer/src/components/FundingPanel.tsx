import { useEffect, useRef, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import { ArrowRight, ChevronRight, Clock3, Copy, Plus, TriangleAlert } from "lucide-react";
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
  if (payment.status === "waiting" && Date.parse(payment.expires_at ?? "") <= now) return "Rate expired";
  return ({creating: "Preparing payment", waiting: "Awaiting payment", confirming: "Confirming", confirmed: "Confirming",
    sending: "Confirming", partially_paid: "Partial payment", finished: "Confirming", failed: "Failed", expired: "Expired", refunded: "Refunded"} as Record<string, string>)[payment.status] ?? "Under review";
}

function awaitingQuote(payment: CryptoPayment): boolean {
  return !payment.review_required && payment.credited_microusd === 0 && ["waiting", "expired"].includes(payment.status);
}

function sentPaymentIds(accountId: string): Set<string> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(`axiom.cryptoSent.${accountId}`) ?? "[]");
    return new Set(Array.isArray(value) ? value.slice(-100).filter((id): id is string => typeof id === "string" && /^[0-9a-f-]{36}$/.test(id)) : []);
  } catch { return new Set(); }
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

export function CryptoPaymentDetails({ payment, options, now = Date.now(), sent = false, refreshing = false, rateUpdated = false }: {
  payment: CryptoPayment; options: CryptoOptions; now?: number; sent?: boolean; refreshing?: boolean; rateUpdated?: boolean;
}) {
  const [copied, setCopied] = useState<string | null>(null);
  const [copyError, setCopyError] = useState(false);
  const generation = useRef(0);
  useEffect(() => { generation.current++; setCopied(null); setCopyError(false); return () => { generation.current++; }; }, [payment.id]);
  const coin = options.currencies.find((item) => item.code === payment.pay_currency);
  const status = awaitingQuote(payment) && sent ? "Checking payment" : awaitingQuote(payment) && refreshing ? "Updating rate…" : cryptoStatus(payment, now);
  const payable = status === "Awaiting payment" && !!coin && !!payment.pay_address && !!payment.pay_amount
    && Date.parse(payment.expires_at ?? "") > now;
  const secondsLeft = Math.max(0, Math.ceil((Date.parse(payment.expires_at ?? "") - now) / 1_000));
  const timeLeft = `${String(Math.floor(secondsLeft / 60)).padStart(2, "0")}:${String(secondsLeft % 60).padStart(2, "0")}`;
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
    {sent && awaitingQuote(payment) ? <p role="status" className="text-[12px] text-[var(--color-text-secondary)]">Waiting for your transfer. Confirmations can take longer.</p> : null}
    {rateUpdated && payable ? <p role="status" className="text-[12px] text-[var(--color-text-secondary)]">Rate updated. Use this amount and address.</p> : null}
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
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-[11px] leading-5 text-[var(--color-text-secondary)]">
        <p>Use {cryptoNetwork(coin!)}.</p>
        <p className="inline-flex items-center gap-1.5"><Clock3 size={13} aria-hidden="true" />Send within <span role="timer" aria-label="Time left to send for this rate" aria-live="off" className="font-medium tabular-nums">{timeLeft}</span> for this rate.</p>
      </div>
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
  const [depositClosed, setDepositClosed] = useState(false);
  const [sentPayments, setSentPayments] = useState<Set<string>>(() => sentPaymentIds(accountId));
  const [renewal, setRenewal] = useState<{sourceId: string; busy: boolean; failed: boolean} | null>(null);
  const [rateUpdatedId, setRateUpdatedId] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now);
  const form = useRef<HTMLFormElement>(null);
  const generation = useRef(0);
  const intent = useRef<CreateCryptoPaymentRequest | null>(null);
  const pending = useRef(false);
  const credited = useRef(new Set<string>());
  const renewals = useRef(new Map<string, {request: CreateCryptoPaymentRequest; running: boolean}>());
  const attemptedRenewals = useRef(new Set<string>());
  const selectedRef = useRef(selected);
  const sentRef = useRef(sentPayments);
  const checkoutRevision = useRef(0);
  selectedRef.current = selected;
  sentRef.current = sentPayments;
  useEffect(() => {
    generation.current++;
    setOptions(null); setOpen(false); setPayments([]); setSelected(null); setError(null); setDepositClosed(false);
    intent.current = null; credited.current.clear();
    renewals.current.clear(); attemptedRenewals.current.clear(); checkoutRevision.current++;
    setSentPayments(sentPaymentIds(accountId)); setRenewal(null); setRateUpdatedId(null);
  }, [accountId]);
  useEffect(() => {
    const current = ++generation.current;
    setBusy(false); pending.current = false;
    setRenewal((previous) => previous?.busy ? {...previous, busy: false, failed: true} : previous);
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

  useEffect(() => {
    if (depositClosed && !selected) form.current?.querySelector<HTMLElement>('[role="combobox"]')?.focus();
  }, [depositClosed, selected]);

  const renewQuote = async () => {
    const api = window.axiomDesktop?.agent;
    const source = selectedRef.current;
    if (!api || !connected || !source || !awaitingQuote(source) || sentRef.current.has(source.id)) return;
    const current = generation.current, revision = checkoutRevision.current;
    let attempt = renewals.current.get(source.id);
    if (!attempt) {
      attempt = {request: {id: crypto.randomUUID(), amountMicrousd: source.amount_microusd, payCurrency: source.pay_currency}, running: false};
      renewals.current.set(source.id, attempt);
    }
    if (attempt.running) return;
    attempt.running = true; attemptedRenewals.current.add(source.id);
    setRenewal({sourceId: source.id, busy: true, failed: false}); setError(null);
    const stillSelected = () => generation.current === current && checkoutRevision.current === revision
      && selectedRef.current?.id === source.id && awaitingQuote(selectedRef.current) && !sentRef.current.has(source.id);
    try {
      // Read stored provider status before issuing a fresh fixed-rate order. Keep
      // the original quote immutable so delayed transfers reconcile independently.
      const history = await api.cryptoPayments(accountId);
      if (!stillSelected()) return;
      setPayments(history.payments);
      const latest = history.payments.find((p) => p.id === source.id);
      if (!latest) throw new Error("Payment status unavailable.");
      if (!awaitingQuote(latest) || Date.parse(latest.expires_at ?? "") > Date.now()) {
        setSelected(latest);
        if (latest.credited_microusd > 0) await onRefreshBilling();
        // A provider-updated deadline can become eligible again later.
        if (awaitingQuote(latest)) attemptedRenewals.current.delete(source.id);
        return;
      }
      const result = await api.createCryptoPayment(attempt.request, accountId);
      if (generation.current !== current) return;
      setPayments((previous) => [result.payment, ...previous.filter((p) => p.id !== result.payment.id)]);
      if (!stillSelected()) return;
      if (awaitingQuote(result.payment) && (result.payment.status !== "waiting"
        || !Number.isFinite(Date.parse(result.payment.expires_at ?? "")) || Date.parse(result.payment.expires_at!) <= Date.now())) {
        throw new Error("No fresh rate returned.");
      }
      setNow(Date.now()); setSelected(result.payment); setRateUpdatedId(result.payment.id);
    } catch {
      if (stillSelected()) setRenewal({sourceId: source.id, busy: false, failed: true});
    } finally {
      attempt.running = false;
      if (generation.current === current) setRenewal((previous) => previous?.sourceId === source.id && previous.busy ? null : previous);
    }
  };

  useEffect(() => {
    if (!open || !connected || !options?.enabled || !selected || !awaitingQuote(selected) || sentPayments.has(selected.id)
      || !Number.isFinite(Date.parse(selected.expires_at ?? "")) || Date.parse(selected.expires_at!) > now
      || attemptedRenewals.current.has(selected.id)) return;
    void renewQuote();
  }, [open, connected, options?.enabled, selected, sentPayments, now]);

  const create = async () => {
    const api = window.axiomDesktop?.agent;
    const value = creditAmount(amount);
    if (!api || !connected || !value || !coin || pending.current) return;
    intent.current ??= {id: crypto.randomUUID(), amountMicrousd: value, payCurrency: coin};
    const current = generation.current;
    pending.current = true; setBusy(true); setError(null); setDepositClosed(false);
    try {
      const result = await api.createCryptoPayment(intent.current, accountId);
      if (generation.current !== current) return;
      setNow(Date.now()); setSelected(result.payment); intent.current = null;
      setPayments((previous) => [result.payment, ...previous.filter((p) => p.id !== result.payment.id)]);
    } catch (failure) { if (generation.current === current) {
      const message = desktopErrorMessage(failure, "Could not confirm payment. Retry safely.");
      setError(message);
      if (message.includes("Increase the amount")) intent.current = null;
    } }
    finally { if (generation.current === current) { pending.current = false; setBusy(false); } }
  };
  const sent = !!selected && sentPayments.has(selected.id);
  const refreshing = renewal?.sourceId === selected?.id && !!renewal?.busy;
  const canCancel = selected && !sent && (selected.status === "creating" || awaitingQuote(selected));
  const rateExpired = selected && awaitingQuote(selected) && Date.parse(selected.expires_at ?? "") <= now;
  const markSent = () => {
    if (!selected) return;
    const updated = new Set(sentRef.current).add(selected.id);
    try { localStorage.setItem(`axiom.cryptoSent.${accountId}`, JSON.stringify([...updated].slice(-100))); } catch { /* Keep the in-memory hint when storage is unavailable. */ }
    sentRef.current = updated; checkoutRevision.current++; setSentPayments(updated); setRenewal(null); setError(null);
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
        {selected ? <><CryptoPaymentDetails payment={selected} options={options} now={now} sent={sent} refreshing={refreshing} rateUpdated={rateUpdatedId === selected.id} />
          {rateExpired && !sent && !refreshing ? <div className="mt-4 space-y-3">
            {renewal?.failed ? <p role="alert" className="text-[12px] text-[var(--color-danger-strong)]">Couldn’t refresh the rate.</p> : null}
            <button type="button" className="ax-pill ax-pill-button w-full justify-center" disabled={!connected} onClick={() => void renewQuote()}>Retry quote</button>
          </div> : null}
          <div className="mt-5 flex gap-2">
            {awaitingQuote(selected) && !sent ? <button type="button" className="ax-pill ax-pill-button flex-1 justify-center" onClick={markSent}>I’ve sent it</button> : null}
            <button type="button" className="ax-pill ax-pill-button flex-1 justify-center" disabled={busy} onClick={() => { checkoutRevision.current++; setSelected(null); intent.current = null; setError(null); setRenewal(null); setRateUpdatedId(null); setDepositClosed(!!canCancel); }}>{canCancel ? "Cancel deposit" : "New payment"}</button>
          </div></> :
          <form ref={form} className="space-y-5" onSubmit={(e) => { e.preventDefault(); void create(); }}>
            {depositClosed ? <p role="status" className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Deposit closed. Funds already sent will still be processed.</p> : null}
            <div><CryptoCurrencyPicker currencies={options.currencies} value={coin} disabled={!connected || busy || !!intent.current}
              onChange={(value) => { intent.current = null; setCoin(value); setError(null); setDepositClosed(false); }} />
            </div>
            <CryptoNetworkWarning coin={options.currencies.find((c) => c.code === coin)} currencies={options.currencies} />
            <div><label htmlFor={`crypto-amount-${accountId}`} className="mb-2 block text-[12px] font-medium text-[var(--color-text-secondary)]">Amount (USD credit)</label>
              <div className="flex min-h-14 items-center gap-2 rounded-xl border border-[var(--color-border)] bg-[var(--wash-row)] px-3.5 transition-colors focus-within:border-[var(--color-border-accent)]">
                <span aria-hidden="true" className="text-[16px] text-[var(--color-text-tertiary)]">$</span>
                <input id={`crypto-amount-${accountId}`} className="min-w-0 flex-1 bg-transparent py-3 text-[18px] font-medium tabular-nums outline-none disabled:opacity-40" inputMode="decimal" value={amount} disabled={!connected || busy || !!intent.current} maxLength={7}
                  aria-invalid={creditAmount(amount) === null} onChange={(e) => { intent.current = null; setAmount(e.target.value); setError(null); setDepositClosed(false); }} />
                <span aria-hidden="true" className="text-[11px] text-[var(--color-text-tertiary)]">USD</span>
              </div>
              {creditAmount(amount) === null ? <p className="mt-2 text-[12px] text-[var(--color-danger-strong)]">Enter $5–$1,000.</p> : null}
            </div>
            <button type="submit" className="flex min-h-12 w-full items-center justify-center gap-2 rounded-xl bg-[var(--color-cherry)] px-4 py-3 text-[13px] font-medium text-on-accent transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.98] disabled:opacity-35" disabled={!connected || busy || !coin || creditAmount(amount) === null}>
              {busy ? "Creating…" : intent.current ? "Retry" : "Create payment"}<ArrowRight size={15} aria-hidden="true" />
            </button>
          </form>}
        {payments.length ? <details className="mt-5 border-t border-[var(--color-border)] pt-4"><summary className="cursor-pointer text-[12px] text-[var(--color-text-secondary)]">Recent payments</summary>
          <ul className="mt-3 space-y-1">{payments.map((p) => <li key={p.id}><button type="button" className="flex w-full items-center gap-2.5 rounded-lg p-2 text-left text-[12px] hover:bg-[var(--wash-row)]" onClick={() => { checkoutRevision.current++; setNow(Date.now()); setSelected(p); intent.current = null; setError(null); setRenewal(null); setRateUpdatedId(null); setDepositClosed(false); }}>
            <CryptoIcon code={p.pay_currency} className="h-5 w-5" /><span className="flex-1 tabular-nums">{usdFromMicrousd(p.amount_microusd, 2)} · {cryptoSymbol(p.pay_currency)}</span><span className="text-[11px] text-[var(--color-text-tertiary)]">{sentPayments.has(p.id) && awaitingQuote(p) ? "Checking payment" : cryptoStatus(p, now)}</span>
          </button></li>)}</ul>
        </details> : null}
        {error ? <p role="alert" className="mt-3 text-[12px] text-[var(--color-danger-strong)]">{error}</p> : null}
        {!connected ? <p role="status" className="mt-3 text-[12px]">Reconnecting…</p> : null}
      </CryptoDepositDialog> : null}
    </section> : null}
  </>;
}

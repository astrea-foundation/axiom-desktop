import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown, CreditCard, ExternalLink, Wallet } from "lucide-react";
import type { AutomaticTopup, BillingOperation, CardFundingOptions, CardFundingPayment, CreditBillingAction, CreditBillingResult, CreditSummary, CreditUsage, SavedBillingCard, SubscriptionPlan } from "@axiom/axiom-acp-client";
import { creditAmount, inputPurchasedCredit, purchasedCreditAmount } from "@axiom/chat-core/creditBalances";
import { desktopErrorMessage } from "../signInFlow";
import { creditAmount as paymentAmount } from "./FundingPanel";
import { usdFromMicrousd } from "../lib/currency";

const button = "ax-pill ax-pill-button disabled:opacity-40";
const field = "w-full rounded-lg border border-[var(--color-border)] bg-[var(--surface-card)] px-3 py-2 text-[13px]";
type BillingValues = { [K in CreditBillingResult["kind"]]: Extract<CreditBillingResult, {kind: K}>["value"] };
const money = (cents: number) => usdFromMicrousd(cents * 10_000, 2);
function capCents(value: string): number | null {
  if (!/^(0|[1-9]\d{0,4})(\.\d{1,2})?$/.test(value)) return null;
  const [whole, fraction = ""] = value.split(".");
  const cents = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
  return cents <= 1_000_000 ? cents : null;
}
function intent(account: string, key: string): string {
  const name = `axiom.billing.${account}.${key}`;
  try {
    const existing = sessionStorage.getItem(name);
    if (existing && /^[0-9a-f-]{36}$/.test(existing)) return existing;
    const id = crypto.randomUUID(); sessionStorage.setItem(name, id); return id;
  } catch { return crypto.randomUUID(); }
}
function retireIntent(account: string, id: string) {
  try {
    for (const name of Object.keys(sessionStorage)) {
      if (name.startsWith(`axiom.billing.${account}.`) && sessionStorage.getItem(name) === id) sessionStorage.removeItem(name);
    }
  } catch { /* The durable native/server operation remains authoritative. */ }
}

/** Credentials and Stripe redirects are handled by the native account client. */
export function CreditBillingPanel({ accountId, connected, cryptoContent, onSummary, onRefreshBilling }: {
  accountId: string; connected: boolean; cryptoContent: ReactNode;
  onSummary: (active: boolean) => void; onRefreshBilling: () => Promise<void>;
}) {
  const [summary, setSummary] = useState<CreditSummary | null>(null);
  const [plans, setPlans] = useState<SubscriptionPlan[]>([]);
  const [options, setOptions] = useState<CardFundingOptions | null>(null);
  const [payments, setPayments] = useState<CardFundingPayment[]>([]);
  const [auto, setAuto] = useState<AutomaticTopup | null>(null);
  const [cards, setCards] = useState<SavedBillingCard[]>([]);
  const [operations, setOperations] = useState<BillingOperation[]>([]);
  const [usage, setUsage] = useState<CreditUsage | null>(null);
  const [tab, setTab] = useState<"card" | "crypto">("card");
  const [amount, setAmount] = useState("10");
  const [threshold, setThreshold] = useState("100");
  const [refill, setRefill] = useState("10");
  const [cap, setCap] = useState("50");
  const [card, setCard] = useState("");
  const [consent, setConsent] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [chosen, setChosen] = useState<SubscriptionPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const unsupportedRef = useRef(false);
  const generation = useRef(0), pending = useRef(false), reading = useRef(false), dirtyRef = useRef(false);
  dirtyRef.current = dirty;
  const call = useCallback(async <K extends CreditBillingResult["kind"]>(action: CreditBillingAction, kind: K): Promise<BillingValues[K]> => {
    const api = window.axiomDesktop?.agent.creditBilling;
    if (!connected) throw new Error("Reconnect to manage billing.");
    if (!api) throw new Error("Restart Axiom with the updated native app to manage billing.");
    const response = await api(action, accountId);
    if (response.result.kind !== kind) throw new Error("Axiom returned an unexpected billing response.");
    return response.result.value as unknown as BillingValues[K];
  }, [accountId, connected]);
  const load = useCallback(async () => {
    if (reading.current) return;
    reading.current = true;
    const current = generation.current;
    try {
      const s = await call({kind: "summary"}, "summary");
      if (current !== generation.current) return;
      setSummary(s); onSummary(!!s);
      if (s?.subscription && !["creating", "pending"].includes(s.subscription.state)) retireIntent(accountId, s.subscription.id);
      const o = await call({kind: "card_options"}, "card_options");
      if (current !== generation.current) return;
      setOptions(o);
      if (o.enabled) {
        const p = await call({kind: "card_payments"}, "card_payments");
        if (current !== generation.current) return;
        setPayments(p);
        for (const payment of p) if (!["creating", "open", "pending"].includes(payment.status)) retireIntent(accountId, payment.id);
      }
      if (s) {
        const u = await call({kind: "usage"}, "usage");
        if (current !== generation.current) return;
        setUsage(u);
      }
      if (s?.capabilities.card) {
        const [p, a, c, history] = await Promise.all([
          call({kind: "plans"}, "plans"), call({kind: "auto_topup"}, "auto_topup"),
          call({kind: "cards"}, "cards"), call({kind: "operations"}, "operations"),
        ]);
        if (current !== generation.current) return;
        setPlans(p.enabled ? p.plans : []); setAuto(a); setCards(c.cards); setOperations(history);
        for (const operation of history) if (["succeeded", "canceled", "failed"].includes(operation.state)) retireIntent(accountId, operation.id);
        if (!dirtyRef.current) {
          setThreshold(purchasedCreditAmount(a.threshold_units, true).replace(/[$,]/g, ""));
          setRefill(String(a.amount_cents / 100)); setCap(String(a.monthly_cap_cents / 100)); setCard(a.payment_method_id ?? "");
        }
      }
      if (current === generation.current) setError(null);
    } catch (e) {
      if (current === generation.current) {
        if (e instanceof Error && /Restart Axiom with the updated native app/.test(e.message)) {
          unsupportedRef.current = true; setUnsupported(true); onSummary(false);
        } else setError(desktopErrorMessage(e, "Could not load billing."));
      }
    } finally { if (current === generation.current) reading.current = false; }
  }, [call, onSummary, accountId]);
  useEffect(() => {
    const current = ++generation.current;
    pending.current = false; reading.current = false;
    unsupportedRef.current = false; setUnsupported(false);
    setSummary(null); setPlans([]); setOptions(null); setPayments([]); setAuto(null); setCards([]); setOperations([]);
    setDirty(false); dirtyRef.current = false; setConsent(false); setBusy(false); setError(null); setNotice(null); setChosen(null); setUsage(null);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (!pending.current) await load();
      if (current === generation.current && !unsupportedRef.current) timer = setTimeout(() => void poll(), 10_000);
    };
    if (connected && window.axiomDesktop?.agent.creditBilling) void poll();
    return () => { generation.current++; if (timer) clearTimeout(timer); };
  }, [accountId, connected, load]);
  const perform = async (work: () => Promise<unknown>, message?: string) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError(null); setNotice(null);
    const current = generation.current;
    try {
      await work();
      if (current !== generation.current) return;
      if (message) setNotice(message);
      await load();
      if (current === generation.current) await onRefreshBilling();
    } catch (e) {
      if (current === generation.current) setError(desktopErrorMessage(e, "Billing could not be updated. Retry to check the same operation."));
    } finally { if (current === generation.current) { pending.current = false; setBusy(false); } }
  };
  const edit = () => { setDirty(true); setConsent(false); };
  const configure = async (enabled: boolean) => {
    if (!auto) return;
    const amountValue = paymentAmount(refill), capValue = capCents(cap);
    let thresholdUnits: string;
    try { thresholdUnits = inputPurchasedCredit(threshold); }
    catch { throw new Error("Enter a balance threshold in dollars, a $5–$1,000 refill, and a monthly cap that covers a refill (maximum $10,000)."); }
    if (!amountValue || capValue === null || capValue < amountValue / 10_000) throw new Error("Enter a balance threshold in dollars, a $5–$1,000 refill, and a monthly cap that covers a refill (maximum $10,000).");
    await call({kind: "configure_auto_topup", settings: {
      enabled, version: auto.version, threshold_units: thresholdUnits, amount_cents: amountValue / 10_000,
      monthly_cap_cents: capValue, payment_method_id: card || null,
      consent_version: auto.consent_version, consent_accepted: enabled && consent,
    }}, "auto_topup");
    dirtyRef.current = false; setDirty(false); setConsent(false);
  };
  if (typeof window === "undefined" || !window.axiomDesktop?.agent.creditBilling) return <>{cryptoContent}</>;
  if (unsupported) return <><p className="mt-4 text-[12px] text-[var(--color-text-secondary)]">Restart with the updated Axiom app to manage cards and subscriptions.</p>{cryptoContent}</>;
  return <div className="space-y-7 text-[13px] text-[var(--color-text-primary)]">
    {summary ? <>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="rounded-2xl bg-[var(--wash-row)] p-4"><h2 className="text-[12px] text-[var(--color-text-secondary)]">Weekly credits</h2>
          <p className="mt-1 text-[30px] font-semibold tabular-nums" title={creditAmount(summary.allowance.remaining_units, true)}>{creditAmount(summary.allowance.remaining_units)}</p>
          {summary.allowance.resets_at ? (() => {
            const pct = Math.min(100, Math.max(0, Number(BigInt(summary.allowance.remaining_units) * 100n / (BigInt(summary.allowance.limit_units) || 1n))));
            return <>
              <div role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={`${creditAmount(summary.allowance.remaining_units, true)} of ${creditAmount(summary.allowance.limit_units, true)} weekly credits remaining`} className="mt-3 h-1.5 w-full overflow-hidden rounded-full bg-[var(--wash-hover)]"><div className="h-full rounded-full bg-[var(--color-text-primary)]" style={{ width: `${pct}%` }} /></div>
              <p className="mt-2 text-[12px] text-[var(--color-text-secondary)]">of {creditAmount(summary.allowance.limit_units, true)} this week</p>
              <p className="mt-1 text-[11px] text-[var(--color-text-tertiary)]">Resets {new Date(summary.allowance.resets_at).toLocaleString()}</p>
            </>;
          })() : <p className="mt-2 text-[12px] text-[var(--color-text-secondary)]">Choose a plan to receive credits every 7 days.</p>}
          {summary.allowance.held_units !== "0" ? <p className="mt-2 text-[11px] text-[var(--color-text-secondary)]">{creditAmount(summary.allowance.held_units, true)} reserved for requests in progress</p> : null}
        </div>
        <div className="rounded-2xl bg-[var(--wash-row)] p-4"><h2 className="text-[12px] text-[var(--color-text-secondary)]">Purchased credit</h2>
          <p className="mt-1 text-[30px] font-semibold tabular-nums" title={purchasedCreditAmount(summary.purchased.available_units, true)}>{purchasedCreditAmount(summary.purchased.available_units)}</p>
          <p className="mt-2 text-[12px] text-[var(--color-text-secondary)]">At API cost.</p>
          {summary.purchased.held_units !== "0" ? <p className="mt-1 text-[11px] text-[var(--color-text-secondary)]">{purchasedCreditAmount(summary.purchased.held_units, true)} reserved for requests in progress</p> : null}
        </div>
      </div>
      {BigInt(summary.trial.posted_units) > 0n ? <p>Trial credits: {creditAmount((BigInt(summary.trial.posted_units) - BigInt(summary.trial.held_units)).toString(), true)}. Used after your weekly allowance.</p> : null}
      {summary.payment_review_required ? <p role="alert" className="text-[var(--color-warning-strong)]">Spending is paused while a payment is reviewed.</p> : null}
      {plans.length ? <section aria-label="Subscription plans"><h3 className="mb-1 font-medium">Choose your plan</h3>
        <p className="mb-3 text-[12px] text-[var(--color-text-secondary)]">Billed monthly. Credits refresh every 7 days.</p>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">{plans.map(plan => { const current = summary.subscription?.plan_key === plan.key; return <div key={plan.key} className={`flex flex-col rounded-xl border p-3 ${current ? "border-[var(--color-border-accent)] bg-[var(--wash-row)]" : "border-[var(--color-border)]"}`}>
          <p className="text-[12px] font-medium">{plan.name}</p>
          <p className="mt-2 text-[22px] font-semibold tabular-nums tracking-[-0.03em]">{money(plan.amount_cents)}<span className="text-[11px] font-normal text-[var(--color-text-tertiary)]">/mo</span></p>
          <p className="mt-1 mb-3 text-[12px] text-[var(--color-text-secondary)]"><span className="font-semibold text-[var(--color-text-primary)]">{creditAmount(plan.weekly_units, true)}</span> credits / week</p>
          <button className={`${button} mt-auto w-full justify-center`} disabled={busy || !connected || current || !!summary.subscription?.pending_plan_key} onClick={() => setChosen(plan)}>{current ? "Current plan" : summary.subscription ? "Change plan" : "Subscribe"}</button>
        </div>; })}</div>
        {chosen ? <div role="dialog" aria-label="Confirm subscription plan" className="mt-3 rounded-xl bg-[var(--wash-row)] p-4">
          <p>{summary.subscription ? `Change to ${chosen.name} at your next monthly renewal for ${money(chosen.amount_cents)} per month.` : `Subscribe to ${chosen.name} for ${money(chosen.amount_cents)} per month with ${creditAmount(chosen.weekly_units, true)} credits each week.`}</p>
          <div className="mt-3 flex gap-2"><button className={button} disabled={busy} onClick={() => void perform(async () => {
            await call({kind: summary.subscription ? "change" : "checkout", id: intent(accountId, `plan.${chosen.key}.${chosen.version}.${summary.subscription?.id ?? "new"}`), plan_key: chosen.key, catalog_version: chosen.version}, "operation"); setChosen(null);
          }, summary.subscription ? "Plan change scheduled. Your current allowance remains available until the change takes effect." : "Complete your subscription in the browser, then return here.")}>Confirm</button>
          <button className={button} disabled={busy} onClick={() => setChosen(null)}>Cancel</button></div>
        </div> : null}
      </section> : null}
      {summary.subscription ? <div className="space-y-2"><p>Subscription: {summary.subscription.state}{summary.subscription.cancel_at_period_end ? " · Cancels at the end of paid coverage" : ""}{summary.subscription.pending_plan_key ? ` · ${summary.subscription.pending_plan_key} at next renewal` : ""}</p>
        <button className={button} disabled={busy || !connected} onClick={() => void perform(() => call({kind: "portal"}, "portal"))}><ExternalLink size={13} />Manage payment method or cancel</button>
        {["creating", "pending"].includes(summary.subscription.state) ? <button className={button} disabled={busy} onClick={() => void perform(() => call({kind: "operation", id: summary.subscription!.id}, "operation"))}>Resume subscription checkout</button> : null}
      </div> : null}
      <label className="flex items-start gap-2"><input type="checkbox" checked={summary.paid_fallback} disabled={busy || !connected} onChange={e => void perform(() => call({kind: "fallback", enabled: e.target.checked}, "summary"))} /><span>Use purchased credit when weekly and trial credits run out.</span></label>
    </> : null}

    <section aria-label="Add credit"><h3 className="mb-3 font-medium">Add credit</h3>
      <div className="mb-4 flex gap-2" role="group" aria-label="Payment method"><button className={button} aria-pressed={tab === "card"} onClick={() => setTab("card")}><CreditCard size={14} />Card</button><button className={button} aria-pressed={tab === "crypto"} onClick={() => setTab("crypto")}><Wallet size={14} />Crypto</button></div>
      {tab === "crypto" ? cryptoContent : options?.enabled ? <>
        <form className="flex flex-wrap items-end gap-3" onSubmit={e => { e.preventDefault(); void perform(async () => {
          const value = paymentAmount(amount); if (!value) throw new Error("Enter an amount between $5 and $1,000 with at most two decimal places.");
          if (value < options.min_amount_microusd || value > options.max_amount_microusd) throw new Error("This amount is outside the available card payment range.");
          await call({kind: "create_card_payment", id: intent(accountId, `card.${value}`), amount_microusd: value}, "card_payment");
        }, "Complete your payment in the browser. Purchased credit appears after payment is confirmed."); }}>
          <label className="min-w-36 flex-1">Amount (USD)<input className={`${field} mt-1`} inputMode="decimal" value={amount} onChange={e => setAmount(e.target.value)} /></label><button className={button} disabled={busy || !connected}>Continue with card <ExternalLink size={13} /></button>
        </form>
        {payments.length ? <details className="mt-3"><summary className="cursor-pointer py-2">Card payments</summary><ul className="space-y-3">{payments.map(p => <li key={p.id} className="flex flex-wrap items-center gap-2"><span>{usdFromMicrousd(p.amount_microusd, 2)} · {p.status}</span>
          {["creating","open","pending"].includes(p.status) ? <><button className={button} disabled={busy} onClick={() => void perform(async () => {
            const value = await call({kind: "check_card_payment", id: p.id}, "card_payment");
            if (value.checkout_url && value.status === "open") await call({kind: "create_card_payment", id: p.id, amount_microusd: p.amount_microusd}, "card_payment");
          })}>Check or resume</button><button className={button} disabled={busy} onClick={() => void perform(() => call({kind: "cancel_card_payment", id: p.id}, "card_payment"))}>Cancel</button></> : null}
        </li>)}</ul></details> : null}
      </> : <p className="text-[var(--color-text-secondary)]">{!connected ? "Reconnect to load payment options." : options ? "Card payments are currently unavailable." : "Loading card payment options…"}</p>}
    </section>

    {auto && (auto.available || auto.enabled) ? <details aria-label="Automatic top-up" className="auto-topup group border-t border-[var(--color-border)] pt-5">
      <summary className="flex cursor-pointer list-none items-center gap-2 text-[14px] font-medium">Automatic top-up <span className="text-[11px] font-normal text-[var(--color-text-tertiary)]">{auto.enabled ? "Enabled" : "Off"}</span><ChevronDown size={14} aria-hidden="true" className="ml-auto text-[var(--color-text-tertiary)] transition-transform group-open:rotate-180" /></summary>
      <div className="mt-3 space-y-3">
      <p className="text-[12px] text-[var(--color-text-secondary)]">Refill when your weekly allowance runs out and purchased credit falls below your balance threshold. Saving a card or subscribing leaves this off.</p>
      <p>{auto.enabled ? "Enabled" : "Off"}{auto.paused_reason ? auto.paused_reason === "authorization_mismatch" ? " · Confirm your dollar-balance settings to resume" : ` · Paused: ${auto.paused_reason.replaceAll("_", " ")}` : ""}. {money(auto.used_cents)} of {money(auto.monthly_cap_cents)} used in {auto.cap_month} (UTC).</p>
      <button className={button} disabled={busy || !auto.available} onClick={() => void perform(() => call({kind: "setup_card", id: intent(accountId, `setup.${cards.length}`)}, "operation"), "Save a card in the browser, then return here.")}>Save a card <ExternalLink size={13} /></button>
      <div className="grid gap-3 sm:grid-cols-2"><label>Balance threshold (USD)<input className={`${field} mt-1`} value={threshold} inputMode="decimal" onChange={e => { edit(); setThreshold(e.target.value); }} /></label>
        <label>Top-up amount (USD)<input className={`${field} mt-1`} value={refill} inputMode="decimal" onChange={e => { edit(); setRefill(e.target.value); }} /></label>
        <label>Monthly cap (USD)<input className={`${field} mt-1`} value={cap} inputMode="decimal" onChange={e => { edit(); setCap(e.target.value); }} /></label>
        <label>Saved card<select aria-label="Saved card" className={`${field} mt-1`} value={card} onChange={e => { edit(); setCard(e.target.value); }}><option value="">Choose a card</option>{cards.map(c => <option key={c.id} value={c.id}>{c.brand} •••• {c.last4}</option>)}</select></label>
      </div>
      <label className="flex items-start gap-2"><input type="checkbox" checked={consent} onChange={e => setConsent(e.target.checked)} /><span>I authorize automatic card charges of ${refill}, up to ${cap} per calendar month (UTC), when my weekly allowance is exhausted and my purchased credit falls below ${threshold}. I can turn this off at any time.</span></label>
      <div className="flex gap-2"><button className={button} disabled={busy || !connected || !auto.available || !consent || !card} onClick={() => void perform(() => configure(true), "Automatic top-up settings saved.")}>{auto.enabled ? "Save automatic top-up" : "Enable automatic top-up"}</button>
        {auto.enabled ? <button className={button} disabled={busy} onClick={() => void perform(async () => {
          await call({kind: "configure_auto_topup", settings: {enabled: false, version: auto.version, threshold_units: auto.threshold_units, amount_cents: auto.amount_cents, monthly_cap_cents: auto.monthly_cap_cents, payment_method_id: auto.payment_method_id, consent_version: auto.consent_version, consent_accepted: false}}, "auto_topup");
        }, "Automatic top-up is off. Any charge already sent to your card will still be reconciled.")}>Turn off</button> : null}</div>
      {operations.filter(o => o.kind === "topup").map(o => <div key={o.id} className="flex flex-wrap items-center gap-2 text-[12px]"><span>{money(o.amount_cents)} · {o.state} · {new Date(o.created_at).toLocaleString()}</span>
        {["creating","pending","requires_action"].includes(o.state) ? <><button className={button} disabled={busy} onClick={() => void perform(() => call({kind: "check", id: o.id}, "operation"))}>Check status</button><button className={button} disabled={busy} onClick={() => void perform(() => call({kind: "cancel_topup", id: o.id}, "operation"))}>Cancel attempt</button></> : null}</div>)}
      </div>
    </details> : null}
    {usage ? <details><summary className="cursor-pointer py-2">Usage</summary><div className="mt-1 space-y-4">
      <div className="space-y-2"><p className="text-[12px] text-[var(--color-text-secondary)]">Weekly &amp; trial credits · {creditAmount(usage.total_units, true)} total</p>
        {usage.models.map(m => <div key={`c:${m.provider}:${m.model_id}`} className="flex justify-between gap-3"><span>{m.model_id} <span className="text-[var(--color-text-tertiary)]">({m.provider})</span></span><span>{creditAmount(m.units, true)} credits</span></div>)}</div>
      {usage.purchased ? <div className="space-y-2"><p className="text-[12px] text-[var(--color-text-secondary)]">Purchased spend · {purchasedCreditAmount(usage.purchased.total_units, true)}</p>
        {usage.purchased.models.map(m => <div key={`p:${m.provider}:${m.model_id}`} className="flex justify-between gap-3"><span>{m.model_id} <span className="text-[var(--color-text-tertiary)]">({m.provider})</span></span><span>{purchasedCreditAmount(m.units, true)}</span></div>)}</div> : null}
    </div></details> : null}
    {notice ? <p role="status" className="text-[12px] text-[var(--color-text-secondary)]">{notice}</p> : null}
    {error ? <p role="alert" className="text-[12px] text-[var(--color-danger-strong)]">{error}</p> : null}
  </div>;
}

/** Shared presentation contract. Credit amounts remain exact decimal strings. */
export interface CreditBalanceSummary {
  format: string; scale: string; revision: string;
  purchased: { currency: "usd"; scale: "100000000"; posted_units: string; held_units: string; available_units: string };
  trial: { posted_units: string; held_units: string };
  allowance: { limit_units: string; remaining_units: string; held_units: string;
    resets_at: string | null; effective_until: string | null; generation: string | null };
  subscription: { plan_key: string; state: string; paid_through_at: string | null;
    cancel_at_period_end: boolean; pending_plan_key: string | null } | null;
  paid_fallback: boolean; payment_review_required: boolean;
}
export function creditAmount(value: string, exact = false): string {
  if (typeof value !== "string" || value === "-0" || !/^-?(0|[1-9]\d{0,15})$/.test(value)) throw new Error("Invalid credit amount");
  const units = BigInt(value), absolute = units < 0n ? -units : units;
  if (absolute > 9_000_000_000_000_000n) throw new Error("Credit amount is outside supported bounds");
  if (!exact && units > 0n && units < 1_000_000n) return "Less than 1";
  const whole = new Intl.NumberFormat("en-US").format(absolute / 1_000_000n);
  const fraction = (absolute % 1_000_000n).toString().padStart(6, "0").replace(/0+$/, "");
  return (units < 0n ? "−" : "") + whole + (exact && fraction ? "." + fraction : "");
}
/** Purchased credit has a fixed USD scale, independent of subscription credits. */
export function purchasedCreditAmount(value: string, exact = false): string {
  creditAmount(value, true);
  const units = BigInt(value), absolute = units < 0n ? -units : units;
  if (!exact && units > 0n && units < 1_000_000n) return "Less than $0.01";
  const whole = new Intl.NumberFormat("en-US").format(absolute / 100_000_000n);
  const fraction = exact ? (absolute % 100_000_000n).toString().padStart(8, "0").replace(/0+$/, "").padEnd(2, "0")
    : ((absolute % 100_000_000n) / 1_000_000n).toString().padStart(2, "0");
  return `${units < 0n ? "−" : ""}$${whole}.${fraction}`;
}
export function inputPurchasedCredit(value: string): string {
  const match = /^(0|[1-9]\d*)(?:\.(\d{1,8}))?$/.exec(value.trim());
  if (!match) throw new Error("Enter a dollar amount with up to eight decimal places.");
  const units = BigInt(match[1]!) * 100_000_000n + BigInt((match[2] ?? "").padEnd(8, "0"));
  if (units <= 0n || units > 9_000_000_000_000_000n) throw new Error("Enter a positive dollar amount.");
  return units.toString();
}
export function validateCreditBalances(value: unknown): asserts value is CreditBalanceSummary {
  if (!value || typeof value !== "object") throw new Error("Invalid credit summary");
  const s = value as CreditBalanceSummary;
  if (s.format !== "credits@3" || s.scale !== "1000000"
      || s.purchased.currency !== "usd" || s.purchased.scale !== "100000000") throw new Error("Unsupported credit summary");
  creditAmount(s.purchased.posted_units, true);
  for (const units of [s.revision, s.purchased.held_units, s.purchased.available_units,
    s.trial.posted_units, s.trial.held_units, s.allowance.limit_units, s.allowance.remaining_units, s.allowance.held_units]) {
    creditAmount(units, true);
    if (BigInt(units) < 0n) throw new Error("Credit amounts cannot be negative");
  }
  if (BigInt(s.trial.held_units) > BigInt(s.trial.posted_units)) throw new Error("Invalid trial allocation");
  if (BigInt(s.allowance.remaining_units) + BigInt(s.allowance.held_units) > BigInt(s.allowance.limit_units)) throw new Error("Invalid allowance allocation");
  const paid = BigInt(s.purchased.posted_units) - BigInt(s.purchased.held_units);
  if (BigInt(s.purchased.available_units) !== (paid > 0n ? paid : 0n)) throw new Error("Invalid purchased credit allocation");
  const date = (v: unknown) => v === null || (typeof v === "string" && Number.isFinite(Date.parse(v)));
  if (!date(s.allowance.resets_at) || !date(s.allowance.effective_until)
      || (s.allowance.generation !== null && typeof s.allowance.generation !== "string")
      || typeof s.paid_fallback !== "boolean" || typeof s.payment_review_required !== "boolean") throw new Error("Invalid credit policy");
  if (s.subscription !== null && (!s.subscription || typeof s.subscription.plan_key !== "string"
      || !["creating", "pending", "active", "past_due", "canceled", "expired", "review"].includes(s.subscription.state)
      || !date(s.subscription.paid_through_at) || typeof s.subscription.cancel_at_period_end !== "boolean"
      || (s.subscription.pending_plan_key !== null && typeof s.subscription.pending_plan_key !== "string"))) throw new Error("Invalid subscription");
}

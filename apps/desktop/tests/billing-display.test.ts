import assert from "node:assert/strict";
import test from "node:test";
import type { BillingStatus, CreditBillingAction, CreditBillingResponse } from "@axiom/axiom-acp-client";
import { hasWeeklyAllowance, type CreditBalanceSummary } from "@axiom/chat-core/creditBalances";
import { creditUsageBreakdown, purchasedUsageBreakdown, showsCreditUsage, type CreditUsageSummary } from "@axiom/chat-core/creditUsage";
import { usageFreshness } from "@axiom/chat-core/usageSpend";
import { balanceLabel } from "../src/renderer/src/components/AccountMenu";
import { desktopCreditUsage } from "../src/renderer/src/creditUsageHost";

type Plan = { limit: string; remaining: string } | null;

function credits(plan: Plan, trialUnits = "632000000"): CreditBalanceSummary {
  return {
    format: "credits@3", scale: "1000000", revision: plan ? "7" : "0",
    purchased: { currency: "usd", scale: "100000000", posted_units: "1000000000", held_units: "0", available_units: "1000000000" },
    trial: { posted_units: trialUnits, held_units: "0" },
    allowance: plan
      ? { limit_units: plan.limit, remaining_units: plan.remaining, held_units: "0", resets_at: "2026-11-23T00:00:00+00:00", effective_until: "2026-12-01T00:00:00+00:00", generation: "window" }
      : { limit_units: "0", remaining_units: "0", held_units: "0", resets_at: null, effective_until: null, generation: null },
    subscription: null, paid_fallback: true, payment_review_required: false,
  };
}

// $0.63 free trial plus $10.00 added, as the server reports it for either ledger.
function billing(summary?: CreditBalanceSummary): BillingStatus {
  return { revision: 1, ledgerSequence: 1, currency: "microUSD", postedMicrousd: 10_632_000, availableMicrousd: 10_632_000,
    trialMicrousd: 632_000, paidMicrousd: 10_000_000, ...(summary ? { credits: summary } : {}) };
}

function usage(weekly: string, trial = "0", purchased = "0", period = "week"): CreditUsageSummary {
  const total = (BigInt(weekly) + BigInt(trial)).toString();
  return { period, scale: "1000000", total_units: total, allocation: { subscription_units: weekly, trial_units: trial },
    models: total === "0" ? [] : [{ provider: "near", model_id: "model", units: total }],
    purchased: { currency: "usd", scale: "100000000", total_units: purchased,
      models: purchased === "0" ? [] : [{ provider: "near", model_id: "model", units: purchased }] } };
}

test("the Billing row shows dollars alone without a plan and never counts trial credit as credits", () => {
  assert.equal(balanceLabel(billing()), "$10.63");
  // The account-menu regression: 632 trial credits were added to weekly credits and shown as "632 credits · $10.00".
  assert.equal(balanceLabel(billing(credits(null))), "$10.63");
  assert.equal(balanceLabel(billing(credits({ limit: "10000000000", remaining: "4200000000" }))), "4,200 credits · $10.63");
  // An exhausted plan still reports weekly credits; trial credit does not fill the gap.
  assert.equal(balanceLabel(billing(credits({ limit: "10000000000", remaining: "0" }))), "0 credits · $10.63");
});

test("a weekly allowance exists only for an open plan window", () => {
  assert.equal(hasWeeklyAllowance(credits(null)), false);
  assert.equal(hasWeeklyAllowance(credits({ limit: "5000000000", remaining: "0" })), true);
  assert.equal(hasWeeklyAllowance({ allowance: { limit_units: "0" } }), false);
  assert.equal(hasWeeklyAllowance({ allowance: { limit_units: "0", resets_at: "2026-11-23T00:00:00+00:00" } }), true);
});

test("credit usage is shown only for weekly plan credits", () => {
  const plan = credits({ limit: "5000000000", remaining: "5000000000" });
  assert.equal(showsCreditUsage(credits(null), usage("0")), false);
  // Trial and purchased spending without a plan are dollar charges.
  assert.equal(showsCreditUsage(credits(null), usage("0", "368000000", "25000000")), false);
  assert.equal(showsCreditUsage(plan, usage("0")), true);
  // A lapsed plan still has weekly credits to report for the period.
  assert.equal(showsCreditUsage(credits(null), usage("1200000")), true);
  // A malformed report stays on the credit path, whose validation rejects it.
  assert.equal(showsCreditUsage(credits(null), { ...usage("0"), allocation: { subscription_units: "-1", trial_units: "0" } }), true);
});

test("the desktop host selects the dollar report unless weekly credits apply", async () => {
  const request = { period: "week" as const, timezone: "UTC" };
  const api = (summary: CreditBalanceSummary | null, report: CreditUsageSummary, calls: CreditBillingAction[] = []) => ({
    creditBilling: async (action: CreditBillingAction): Promise<CreditBillingResponse> => {
      calls.push(action);
      return { result: action.kind === "summary" ? { kind: "summary", value: summary } : { kind: "usage", value: report } } as CreditBillingResponse;
    },
  });
  assert.equal(await desktopCreditUsage(undefined, "account", request), null);
  assert.equal(await desktopCreditUsage({}, "account", request), null);
  assert.equal(await desktopCreditUsage(api(null, usage("0")), "account", request), null);
  // An account outside the credit ledger: a preview summary and an all-zero credit report.
  assert.equal(await desktopCreditUsage(api(credits(null), usage("0")), "account", request), null);
  const calls: CreditBillingAction[] = [];
  const weekly = usage("1500000", "0", "125000000");
  assert.deepEqual(await desktopCreditUsage(api(credits({ limit: "5000000000", remaining: "4000000000" }), weekly, calls), "account", request), weekly);
  assert.deepEqual(calls, [{ kind: "summary" }, { kind: "usage", period: "week", timezone: "UTC", allowance_only: true }]);
  const legacyCombined = usage("1500000", "500000", "125000000");
  assert.equal(await desktopCreditUsage(api(credits({ limit: "5000000000", remaining: "4000000000" }), legacyCombined), "account", request), null);
  assert.equal(await desktopCreditUsage({ creditBilling: async () => { throw new Error("Restart Axiom with the updated native app to manage subscriptions and credits."); } }, "account", request), null);
  await assert.rejects(desktopCreditUsage({ creditBilling: async () => { throw new Error("offline"); } }, "account", request), /offline/);
  await assert.rejects(desktopCreditUsage({
    creditBilling: async (action) => ({ result: { kind: "summary", value: action.kind === "summary" ? credits(null) : null } }) as CreditBillingResponse,
  }, "account", request), /Invalid credit usage response/);
});

test("the usage age is measured from the completed fetch", () => {
  const fetched = Date.UTC(2026, 10, 21, 12, 0, 0);
  assert.equal(usageFreshness(fetched, fetched), "Updated just now.");
  assert.equal(usageFreshness(fetched, fetched + 59_000), "Updated just now.");
  assert.equal(usageFreshness(fetched, fetched + 60_000), "Updated 1 min ago.");
  assert.equal(usageFreshness(fetched, fetched + 59 * 60_000), "Updated 59 min ago.");
  assert.match(usageFreshness(fetched, fetched + 60 * 60_000), /^Updated at \d{1,2}:00.*\.$/);
  // A clock that moved backwards never reports a negative age.
  assert.equal(usageFreshness(fetched, fetched - 5_000), "Updated just now.");
});

test("image generation is named in both credit breakdowns and other models keep their id", () => {
  const usage = {
    period: "week", scale: "1000000", total_units: "5000000", allocation: { subscription_units: "5000000", trial_units: "0" },
    models: [{ provider: "axiom-direct", model_id: "direct:image-generation", units: "3000000" }, { provider: "near", model_id: "glm", units: "2000000" }],
    purchased: { currency: "usd", scale: "100000000", total_units: "40000000", models: [{ provider: "axiom-pool", model_id: "pool:image-generation", units: "40000000" }] },
  };
  assert.deepEqual(creditUsageBreakdown(usage, "week").models.map((m) => [m.label, m.provider, m.amount]),
    [["Image generation", "axiom-direct", 3000000n], ["glm", "near", 2000000n]]);
  assert.deepEqual(purchasedUsageBreakdown(usage, "week").models.map((m) => m.label), ["Image generation"]);
});

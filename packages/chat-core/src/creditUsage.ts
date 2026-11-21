import { hasWeeklyAllowance, type WeeklyAllowanceView } from "./creditBalances.js";
import { spendingBreakdown, type UsagePeriod } from "./usageSpend.js";

export interface CreditUsageSummary {
  period: string; scale: string; total_units: string;
  allocation: {subscription_units: string; trial_units: string};
  models: { provider: string; model_id: string; units: string }[];
  purchased: {currency: string; scale: string; total_units: string;
    models: { provider: string; model_id: string; units: string }[]};
}

/**
 * Weekly plan credits are the only spending counted in credits. Without a plan,
 * and with no weekly credits used in the period, every charge is a dollar charge
 * (free trial or added funds), and an account outside the credit ledger reports
 * no credit usage at all. Hosts then return null and show the dollar report.
 */
export function showsCreditUsage(balances: WeeklyAllowanceView, usage: CreditUsageSummary): boolean {
  const weekly = usage?.allocation?.subscription_units;
  // Malformed reports stay on the credit path, whose validation rejects them.
  if (typeof weekly !== "string" || !/^(0|[1-9]\d{0,38})$/.test(weekly)) return true;
  return hasWeeklyAllowance(balances) || BigInt(weekly) > 0n;
}

/**
 * The credit report carries ids only. Image generation is billed under its own
 * id on every worker, so it gets the same name the dollar report sends.
 */
const IMAGE_GENERATION_IDS = new Set(["direct:image-generation", "pool:image-generation"]);
const creditModel = (m: { provider: string; model_id: string; units: string }) => ({
  provider: m.provider, modelId: m.model_id, costMicrousd: m.units,
  ...(IMAGE_GENERATION_IDS.has(m.model_id) ? { modelName: "Image generation" } : {}),
});

/** The chart uses integer millionths internally and keeps its denomination explicit. */
export function creditUsageBreakdown(summary: CreditUsageSummary, period: UsagePeriod = "all_time") {
  if (summary?.scale !== "1000000" || !Array.isArray(summary.models)) throw new Error("Invalid credit usage");
  const amounts = Object.values(summary.allocation);
  if (amounts.length !== 2 || amounts.some(v => !/^(0|[1-9]\d{0,38})$/.test(v))
      || amounts.reduce((sum, v) => sum + BigInt(v), 0n) !== BigInt(summary.total_units)) throw new Error("Invalid credit usage allocation");
  return spendingBreakdown({period: summary.period, totalCostMicrousd: summary.total_units,
    models: summary.models.map(creditModel)}, period);
}

/** Geometry uses raw fixed USD units; callers must format with purchasedCreditAmount. */
export function purchasedUsageBreakdown(summary: CreditUsageSummary, period: UsagePeriod = "all_time") {
  const purchased = summary?.purchased;
  if (purchased?.currency !== "usd" || purchased.scale !== "100000000") throw new Error("Invalid purchased-credit usage");
  return spendingBreakdown({period: summary.period, totalCostMicrousd: purchased.total_units,
    models: purchased.models.map(creditModel)}, period);
}

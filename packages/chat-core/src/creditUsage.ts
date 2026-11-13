import { spendingBreakdown, type UsagePeriod } from "./usageSpend.js";

export interface CreditUsageSummary {
  period: string; scale: string; total_units: string;
  allocation: {subscription_units: string; trial_units: string};
  models: { provider: string; model_id: string; units: string }[];
  purchased: {currency: string; scale: string; total_units: string;
    models: { provider: string; model_id: string; units: string }[]};
}

/** The chart uses integer millionths internally and keeps its denomination explicit. */
export function creditUsageBreakdown(summary: CreditUsageSummary, period: UsagePeriod = "all_time") {
  if (summary?.scale !== "1000000" || !Array.isArray(summary.models)) throw new Error("Invalid credit usage");
  const amounts = Object.values(summary.allocation);
  if (amounts.length !== 2 || amounts.some(v => !/^(0|[1-9]\d{0,38})$/.test(v))
      || amounts.reduce((sum, v) => sum + BigInt(v), 0n) !== BigInt(summary.total_units)) throw new Error("Invalid credit usage allocation");
  return spendingBreakdown({period: summary.period, totalCostMicrousd: summary.total_units,
    models: summary.models.map(m => ({provider: m.provider, modelId: m.model_id, costMicrousd: m.units}))}, period);
}

/** Geometry uses raw fixed USD units; callers must format with purchasedCreditAmount. */
export function purchasedUsageBreakdown(summary: CreditUsageSummary, period: UsagePeriod = "all_time") {
  const purchased = summary?.purchased;
  if (purchased?.currency !== "usd" || purchased.scale !== "100000000") throw new Error("Invalid purchased-credit usage");
  return spendingBreakdown({period: summary.period, totalCostMicrousd: purchased.total_units,
    models: purchased.models.map(m => ({provider: m.provider, modelId: m.model_id, costMicrousd: m.units}))}, period);
}

import type { UsageSummaryRequest, UsageSummary } from "@axiom/axiom-acp-client";

export type UsagePeriod = NonNullable<UsageSummaryRequest["period"]>;

export interface ModelSpending {
  id: string;
  label: string;
  provider: string;
  amount: bigint;
  share: number;
}

function amount(value: unknown): bigint {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]{0,38})$/.test(value)) {
    throw new Error("Spending data is unavailable. Please try again.");
  }
  return BigInt(value);
}

function label(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value !== "string" || !value.trim() || value.length > 512 || /[\u0000-\u001f\u007f]/.test(value)) {
    throw new Error("Spending data is unavailable. Please try again.");
  }
  return value.trim();
}

/** Costs stay integer millionths through validation, merging, sorting and display. */
export function spendingBreakdown(summary: UsageSummary, period: UsagePeriod = "all_time"): { total: bigint; models: ModelSpending[] } {
  if (!summary || summary.period !== period || !Array.isArray(summary.models) || summary.models.length > 512) {
    throw new Error("Spending data is unavailable. Please try again.");
  }
  const total = amount(summary.totalCostMicrousd);
  let sum = 0n;
  const grouped = new Map<string, Omit<ModelSpending, "share">>();
  for (const row of summary.models) {
    const cost = amount(row.costMicrousd);
    const provider = label(row.provider);
    if (!provider) throw new Error("Spending data is unavailable. Please try again.");
    const modelId = label(row.modelId);
    if (!modelId) throw new Error("Spending data is unavailable. Please try again.");
    const modelName = label(row.modelName);
    const id = JSON.stringify([provider, modelId]);
    const current = grouped.get(id);
    grouped.set(id, { id, provider, label: current?.label ?? modelName ?? modelId, amount: (current?.amount ?? 0n) + cost });
    sum += cost;
  }
  if (sum !== total) throw new Error("Spending totals don’t match. Please refresh.");
  const models = [...grouped.values()].filter(row => row.amount > 0n)
    .sort((a, b) => a.amount === b.amount ? a.id.localeCompare(b.id) : a.amount > b.amount ? -1 : 1)
    .map(row => ({ ...row, share: Number(row.amount * 1_000_000n / total) / 1_000_000 }));
  return { total, models };
}

export function formatSpend(value: bigint): string {
  const dollars = (value / 1_000_000n).toLocaleString("en-US");
  const fraction = (value % 1_000_000n).toString().padStart(6, "0").replace(/0+$/, "").padEnd(2, "0");
  return `$${dollars}.${fraction}`;
}

export function shareLabel(model: ModelSpending): string {
  return model.share < 0.001 ? "<0.1%" : `${(model.share * 100).toFixed(1).replace(/\.0$/, "")}%`;
}

export function pieSlice(start: number, end: number): string {
  const point = (fraction: number) => {
    const angle = fraction * 2 * Math.PI - Math.PI / 2;
    return `${120 + 106 * Math.cos(angle)} ${120 + 106 * Math.sin(angle)}`;
  };
  return `M 120 120 L ${point(start)} A 106 106 0 ${end - start > 0.5 ? 1 : 0} 1 ${point(end)} Z`;
}

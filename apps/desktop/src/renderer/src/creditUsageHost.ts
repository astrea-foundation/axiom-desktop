import { showsCreditUsage, type CreditUsageSummary } from "@axiom/chat-core/creditUsage";
import type { UsageSummaryRequest } from "@axiom/chat-core/viewTypes";
import type { AgentApi } from "../../preload/agent-api";

/**
 * Credit usage for Settings → Usage, or null when the dollar report applies:
 * credit billing is unavailable, or the account has no weekly plan credits to
 * report. An account outside the credit ledger is still charged in dollars and
 * returns an all-zero credit report, which must never hide its dollar spending.
 */
export async function desktopCreditUsage(
  api: Pick<AgentApi, "creditBilling"> | undefined,
  account: string,
  request: Pick<UsageSummaryRequest, "period" | "timezone">,
): Promise<CreditUsageSummary | null> {
  if (!api?.creditBilling) return null;
  let balances;
  try {
    const summary = await api.creditBilling({kind: "summary"}, account);
    if (summary.result.kind !== "summary" || !summary.result.value) return null;
    balances = summary.result.value;
  } catch (error) {
    if (error instanceof Error && /Restart Axiom with the updated native app/.test(error.message)) return null;
    throw error;
  }
  const response = await api.creditBilling({kind: "usage", period: request.period ?? "all_time", timezone: request.timezone ?? "UTC"}, account);
  if (response.result.kind !== "usage") throw new Error("Invalid credit usage response");
  return showsCreditUsage(balances, response.result.value) ? response.result.value : null;
}

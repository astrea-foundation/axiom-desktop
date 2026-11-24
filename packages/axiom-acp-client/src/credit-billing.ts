import type { CreditBillingAction } from "./generated/protocol.js";
import { ProtocolError } from "./errors.js";

const keys: Record<CreditBillingAction["kind"], readonly string[]> = {
  summary: [], plans: [], prices: [], usage: ["period", "timezone", "allowance_only"], checkout: ["id", "plan_key", "catalog_version"], change: ["id", "plan_key", "catalog_version"],
  balance_plan_options: [], purchase_balance_plan: ["id", "plan_key", "catalog_version", "coverage_end_on",
    "renewal_consent_version", "renewal_consent_accepted"], set_balance_plan_renewal: ["id", "enabled"],
  portal: [], setup_card: ["id"], cards: [], auto_topup: [], configure_auto_topup: ["settings"],
  fallback: ["enabled"], operations: [], operation: ["id"], check: ["id"], cancel_topup: ["id"],
  card_options: [], create_card_payment: ["id", "amount_microusd"], card_payments: [],
  check_card_payment: ["id"], cancel_card_payment: ["id"],
};
export function validateCreditAction(value: unknown): CreditBillingAction {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new ProtocolError("Invalid billing operation");
  const v = value as Record<string, unknown>, allowed = keys[v.kind as CreditBillingAction["kind"]];
  if (!allowed || Object.keys(v).some(k => k !== "kind" && !allowed.includes(k)) || JSON.stringify(v).length > 8192) throw new ProtocolError("Invalid billing operation");
  if (allowed.includes("id") && (typeof v.id !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(v.id))) throw new ProtocolError("Invalid billing operation ID");
  if (allowed.includes("plan_key") && !["starter","standard","plus","max"].includes(String(v.plan_key))) throw new ProtocolError("Invalid subscription plan");
  if (allowed.includes("catalog_version") && (typeof v.catalog_version !== "string" || !v.catalog_version || v.catalog_version.length>80 || /[\x00-\x1f]/.test(v.catalog_version))) throw new ProtocolError("Invalid plan pricing version");
  if (v.kind === "purchase_balance_plan") {
    if (typeof v.coverage_end_on !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(v.coverage_end_on)) throw new ProtocolError("Invalid plan coverage date");
    if (v.renewal_consent_version !== "balance-renew-v1" || v.renewal_consent_accepted !== true) throw new ProtocolError("Accept the terms for automatic balance renewal to subscribe.");
  }
  if (v.kind === "set_balance_plan_renewal" && typeof v.enabled !== "boolean") throw new ProtocolError("Invalid balance renewal setting");
  if (v.kind === "fallback" && typeof v.enabled !== "boolean") throw new ProtocolError("Invalid credit fallback setting");
  if (v.kind === "usage" && ((v.period != null && !["week", "month", "all_time"].includes(String(v.period)))
      || (v.timezone != null && (typeof v.timezone !== "string" || !v.timezone || v.timezone.length > 128 || /[\x00-\x1f]/.test(v.timezone)))
      || (v.allowance_only != null && typeof v.allowance_only !== "boolean"))) throw new ProtocolError("Invalid credit usage options");
  if (v.kind === "create_card_payment" && (!Number.isSafeInteger(v.amount_microusd) || Number(v.amount_microusd) < 5_000_000 || Number(v.amount_microusd) > 1_000_000_000 || Number(v.amount_microusd) % 10000)) throw new ProtocolError("Invalid card top-up amount");
  return value as CreditBillingAction;
}
export function allowedBillingUrl(value: string): boolean {
  try {
    const url = new URL(value);
    const portalSecrets = url.pathname === "/p/session" ? url.searchParams.getAll("secret") : [];
    const portalPath = url.pathname.startsWith("/p/session/") ||
      (portalSecrets.length === 1 && portalSecrets[0] !== "");
    return value.length <= 4096 && !/[\x00-\x20]/.test(value) && url.protocol === "https:" && !url.username && !url.password && !url.port
      && ((url.hostname === "checkout.stripe.com" && /^\/[cg]\/pay\//.test(url.pathname)) ||
          (url.hostname === "billing.stripe.com" && portalPath));
  } catch { return false; }
}

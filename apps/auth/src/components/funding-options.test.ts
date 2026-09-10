import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import type { BillingStatus } from "../api/types";
import { FundingOptions } from "./FundingOptions";

const billing: BillingStatus = {
  available_microusd: 0, posted_microusd: 0, held_microusd: 0, ledger_sequence: 0,
  currency: "microUSD", invoices_enabled: false, invoice_min_usd_cents: 100,
  invoice_max_usd_cents: 10000, settlement_asset: null,
  payment_account: { network: "mainnet", asset: "ZEC", conversion_status: "none", valuation_enabled: true },
};
function render(value: BillingStatus | null) {
  return renderToStaticMarkup(createElement(FundingOptions, { billing: value, busy: null, onInvoice() {} }));
}
it("shows mainnet deposit valuation guidance without invoice actions", () => {
  const html = render(billing);
  expect(html).toContain("Axiom Desktop");
  expect(html).toContain("mainnet");
  expect(html).toContain("valued in USD to fund inference credit");
  expect(html).toContain("funds remain in ZEC");
  expect(html).not.toContain("testnet");
  expect(html).not.toContain("<button");
  expect(html).not.toContain("Shieldz");
});
it("does not promise inference credit while valuation is disabled", () => {
  const html = render({ ...billing, payment_account: { ...billing.payment_account!, valuation_enabled: false } });
  expect(html).toContain("Deposit credit is currently unavailable");
  expect(html).not.toContain("valued in USD");
});
it("offers invoice actions only when explicitly enabled without persistent deposits", () => {
  expect(render(null)).not.toContain("<button");
  expect(render({ ...billing, payment_account: null })).not.toContain("<button");
  expect(render({ ...billing, payment_account: null, invoices_enabled: true })).toContain("<button");
  expect(render({ ...billing, invoices_enabled: true })).not.toContain("<button");
});

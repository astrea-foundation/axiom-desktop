import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { BillingInvoice } from "@axiom/axiom-acp-client";
import { BalanceScreen } from "../src/renderer/src/components/BalanceScreen.js";

function render(invoice: BillingInvoice | null = null, signedIn = true, split = false) {
  return renderToStaticMarkup(createElement(BalanceScreen, {
    connected: true,
    account: signedIn
      ? { revision: 1, state: "valid", account: { id: "account-a", linkedMethods: ["password"] } }
      : { revision: 2, state: "signed_out" },
    billing: {
      revision: 1, postedMicrousd: 0, heldMicrousd: 0, availableMicrousd: 0,
      ledgerSequence: 0, currency: "microUSD", invoicesEnabled: true,
      invoiceMinUsdCents: 500, invoiceMaxUsdCents: 100_000, settlementAsset: "USDC",
      ...(split ? { postedMicrousd: 3_000_000, availableMicrousd: 3_000_000, trialMicrousd: 1_000_000, paidMicrousd: 2_000_000 } : {}),
    },
    invoice,
    onClose: () => {},
    onRefreshBilling: async () => {}, onRefreshInvoice: async () => {}, onTopUp: async () => {},
    onLogin: () => {},
  }));
}

test("balance distinguishes revocable trial credit from other credit and ZEC", () => {
  const html = render(null, true, true);
  assert.match(html, /Credit breakdown/);
  assert.match(html, /Free trial credit/);
  assert.match(html, /\$1\.00/);
  assert.match(html, /Other credit/);
  assert.match(html, /\$2\.00/);
  assert.match(html, /Trial credit is used first/);
  assert.doesNotMatch(render(), /Free trial credit/);
});

test("billing presents one Top up action, not separate invoice creation/payment steps", () => {
  const html = render();
  assert.match(html, /<form/);
  assert.match(html, /type="submit"[^>]*>Top up<\/button>/);
  assert.match(html, /Shieldz checkout opens in your browser/);
  assert.doesNotMatch(html, /Create invoice|Pay invoice|Reopen checkout/);
});

test("pending top-up shows waiting status and a recovery checkout action", () => {
  const html = render({
    revision: 1, id: "a".repeat(32), amountUsdCents: 2_500, state: "pending",
    payUrl: "https://shieldz.cash/pay/test_invoice", expiresAt: "2030-01-01T00:00:00.000Z",
  });
  assert.match(html, /Waiting for payment/);
  assert.match(html, /Reopen checkout/);
  assert.match(html, /role="status"/);
  assert.doesNotMatch(html, /Create invoice|Pay invoice/);
});

test("paid and signed-out views cannot offer a checkout recovery action", () => {
  const invoice: BillingInvoice = {
    revision: 2, id: "a".repeat(32), amountUsdCents: 2_500, state: "paid", payUrl: null,
  };
  assert.match(render(invoice), /Payment confirmed/);
  assert.doesNotMatch(render(invoice), /Reopen checkout/);
  assert.doesNotMatch(render(invoice, false), /Top up|Reopen checkout|Payment confirmed/);
});

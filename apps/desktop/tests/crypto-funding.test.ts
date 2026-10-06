import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { CryptoOptions, CryptoPayment } from "@axiom/axiom-acp-client";
import { creditAmount, CryptoPaymentDetails, cryptoStatus } from "../src/renderer/src/components/FundingPanel.js";

const options: CryptoOptions = {enabled: true, zcash_discount_bps: 500, min_amount_microusd: 5_000_000,
  max_amount_microusd: 1_000_000_000, currencies: [{code: "usdttrc20", name: "USDT", network: "TRON (TRC20)"}]};
const payment: CryptoPayment = {id: "00000000-0000-4000-8000-000000000001", status: "waiting",
  amount_microusd: 25_000_000, credited_microusd: 0, pay_currency: "usdttrc20", pay_amount: "25.123456789",
  pay_address: "TtestAddress", payin_extra_id: "0012345", expires_at: "2026-10-05T18:30:00Z", review_required: false,
  created_at: "2026-10-05T18:00:00Z"};
const now = Date.parse("2026-10-05T18:15:00Z");

test("credit input uses whole cents and bounds without rounding", () => {
  assert.equal(creditAmount("25.01"), 25_010_000);
  assert.equal(creditAmount("5"), 5_000_000);
  assert.equal(creditAmount("1000"), 1_000_000_000);
  for (const value of ["4.99", "1000.01", "25.001", "1e2", "-25", "NaN", "25.", ""]) assert.equal(creditAmount(value), null);
});

test("payment shows exact amount, network and memo with a local QR", () => {
  const html = renderToStaticMarkup(createElement(CryptoPaymentDetails, {payment, options, now}));
  assert.match(html, /25\.123456789/);
  assert.match(html, /TRON \(TRC20\)/);
  assert.match(html, /Memo: 0012345/);
  assert.match(html, /Copy memo/);
  assert.match(html, /<svg/);
  assert.doesNotMatch(html, /(?:src|href)="https?:\/\//);
});

test("expired, partial, confirming, reviewed and credited payments hide payable details", () => {
  for (const status of ["expired", "partially_paid", "confirming", "review", "refunded"]) {
    const html = renderToStaticMarkup(createElement(CryptoPaymentDetails, {payment: {...payment, status}, options, now}));
    assert.doesNotMatch(html, /aria-label="Crypto payment address"|Copy address|TtestAddress/);
  }
  assert.equal(cryptoStatus({...payment, status: "refunded", credited_microusd: 25_000_000}, now), "Refunded");
  assert.equal(cryptoStatus(payment, Date.parse(payment.expires_at!)), "Expired");
  const html = renderToStaticMarkup(createElement(CryptoPaymentDetails, {payment, options, now: Date.parse(payment.expires_at!)}));
  assert.match(html, /Expired/); assert.doesNotMatch(html, /aria-label="Crypto payment address"|Copy address/);
});

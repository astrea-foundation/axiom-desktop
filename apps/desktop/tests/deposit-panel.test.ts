import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { PaymentAccount } from "@axiom/axiom-acp-client";
import { DepositPanel, zecFromZatoshis } from "../src/renderer/src/components/DepositPanel.js";
import { BalanceScreen } from "../src/renderer/src/components/BalanceScreen.js";

const address = `u1${"a".repeat(180)}`;
const payment: PaymentAccount = {
  network: "mainnet", asset: "ZEC", conversion_status: "none", state: "ready",
  address, payment_uri: `zcash:${address}`, monitoring_status: "ready", required_confirmations: "10",
  confirmed_zatoshis: "9007199254740993", confirming_zatoshis: "1", review_required: false,
  deposits: [{ id: "deposit-one", amount_zatoshis: "100000001", state: "confirmed",
    object_version: "9007199254740993", confirmations: "10", required_confirmations: "10",
    review_required: false, observed_at: "2026-09-07T10:00:00Z" }],
};

test("ZEC formatting never passes through floating point", () => {
  assert.equal(zecFromZatoshis("9007199254740993"), "90071992.54740993");
  assert.equal(zecFromZatoshis("1"), "0.00000001");
  assert.equal(zecFromZatoshis("0"), "0");
  assert.equal(zecFromZatoshis("100000000"), "1");
  for (const invalid of ["01", "1.5", "-1", "NaN", "1e8"]) {
    assert.throws(() => zecFromZatoshis(invalid));
  }
});

test("native checkout shows exact reusable address and a locally rendered QR", () => {
  const html = renderToStaticMarkup(createElement(DepositPanel, { payment, connected: true }));
  assert.match(html, /<svg/);
  assert.match(html, /Mainnet Zcash deposit payment URI/);
  assert.ok(html.includes(address));
  assert.match(html, /1\.00000001 ZEC/);
  assert.match(html, /Reusable address/);
  assert.doesNotMatch(html, /Copy payment URI|Receiver status/);
  assert.doesNotMatch(html, /Deposit details|No minimum or memo|A chain reorganization/);
  assert.doesNotMatch(html, /<img|https?:\/\/[^" ]*(qr|checkout)|<form|type="submit"/);
});

test("provisioning and review never show a payable stale QR", () => {
  const html = renderToStaticMarkup(createElement(DepositPanel, {
    payment: { ...payment, state: "provisioning", review_required: true }, connected: false,
  }));
  assert.match(html, /Preparing your address/);
  assert.match(html, /under review/);
  assert.match(html, /Deposit updates paused while offline/);
  assert.doesNotMatch(html, /Mainnet Zcash deposit payment URI|Copy address/);
});

test("old testnet state cannot be relabeled or displayed as a mainnet payment", () => {
  const html = renderToStaticMarkup(createElement(DepositPanel, {
    payment: { ...payment, network: "testnet", address: `utest1${"a".repeat(180)}` }, connected: true,
  }));
  assert.match(html, /Zcash deposits are unavailable/);
  assert.doesNotMatch(html, /utest1|<svg|Copy address|Receive ZEC/);
});

test("balance screen selects native deposits and hides invoice checkout", () => {
  const props = {
    connected: true,
    account: { revision: 1, state: "valid" as const, account: { id: "account-a", linkedMethods: [] } },
    billing: {
      revision: 1, postedMicrousd: 0, availableMicrousd: 0,
      ledgerSequence: 0, currency: "microUSD", paymentAccount: payment,
    },
    onClose: () => {},
    onRefreshBilling: async () => {},
    onLogin: () => {},
  };
  const html = renderToStaticMarkup(createElement(BalanceScreen, props));
  assert.match(html, /Receive ZEC/);
  assert.doesNotMatch(html, /Shieldz|Opening checkout|top-up-amount/);
  const signedOut = renderToStaticMarkup(createElement(BalanceScreen, {
    ...props, account: { revision: 2, state: "signed_out" },
  }));
  assert.doesNotMatch(signedOut, /Receive ZEC|u1/);
});

test("valued deposits show USD credit and rate, while unpriced confirmations stay pending", () => {
  const valued: PaymentAccount = { ...payment, valuation_enabled: true, deposits: [
    { ...payment.deposits[0], valuation_status: "credited", credit_microusd: "10000000",
      price_microusd_per_zec: "100000000", price_source: "coinbase", priced_at: "2026-09-07T10:00:00Z" },
    { ...payment.deposits[0], id: "deposit-two", valuation_status: "pending" },
  ] };
  const html = renderToStaticMarkup(createElement(DepositPanel, { payment: valued, connected: true }));
  assert.match(html, /Top up via Zcash/);
  assert.match(html, /\$10\.00 credit added/);
  assert.match(html, /\$100\.00\/ZEC/);
  assert.doesNotMatch(html, /Coinbase|Kraken/);
  assert.match(html, /Calculating credit/);
  assert.doesNotMatch(html, /tracked separately from inference credit/);
});

test("ZEC stays primary with a current USD estimate or posted credit rounded to cents", () => {
  const now = Date.now();
  const quote = { source: "coinbase", price_microusd_per_zec: "123456789",
    as_of: new Date(now - 1_000).toISOString(), expires_at: new Date(now + 59_000).toISOString() };
  const pending: PaymentAccount = { ...payment, valuation_enabled: true, deposits: [
    { ...payment.deposits[0], state: "confirming", confirmations: "6", amount_zatoshis: "80000" },
  ] };
  const render = (props = {}) => renderToStaticMarkup(createElement(DepositPanel, { payment: pending, connected: true, quote, ...props }));
  const html = render();
  assert.match(html, /0\.0008 ZEC/);
  assert.match(html, /~\$0\.10/);
  assert.match(html, /6\/10.*confirmations/);
  for (const props of [{ connected: false }, { quote: null },
    { quote: { ...quote, expires_at: new Date(now - 1).toISOString() } }]) {
    assert.doesNotMatch(render(props), /~\$/);
    assert.match(render(props), /0\.0008 ZEC/);
  }
  const confirmed = render({ payment: { ...pending, deposits: [{ ...pending.deposits[0],
    state: "confirmed", confirmations: "12", valuation_status: "credited", credit_microusd: "100001" }] } });
  assert.match(confirmed, /0\.0008 ZEC/);
  assert.match(confirmed, /\$0\.10/);
  assert.doesNotMatch(confirmed, /~\$/);
  assert.match(confirmed, /10\/10.*confirmations/);
});

test("a payment risk hold explains why existing credit is unavailable", () => {
  const html = renderToStaticMarkup(createElement(BalanceScreen, {
    connected: true, account: { revision: 1, state: "valid", account: { id: "a", linkedMethods: [] } },
    billing: { revision: 1, postedMicrousd: 3_000_000, availableMicrousd: 0,
      trialMicrousd: 1_000_000, paidMicrousd: 2_000_000, paymentReviewRequired: true,
      ledgerSequence: 1, currency: "microUSD", paymentAccount: { ...payment, valuation_enabled: true } }, onClose() {}, async onRefreshBilling() {}, async onTopUp() {},
    onLogin() {},
  }));
  assert.match(html, /Spending is paused/);
  assert.match(html, /Free trial credit/);
  assert.match(html, /\$1\.00/);
});

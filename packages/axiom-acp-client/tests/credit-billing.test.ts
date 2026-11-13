import assert from "node:assert/strict";
import test from "node:test";
import { AxiomAcpClient } from "../src/client.js";
import { allowedBillingUrl, validateCreditAction } from "../src/credit-billing.js";
import { creditAmount, purchasedCreditAmount, inputPurchasedCredit, validateCreditBalances, type CreditBalanceSummary } from "@axiom/chat-core/creditBalances";
import { creditUsageBreakdown, purchasedUsageBreakdown } from "@axiom/chat-core/creditUsage";
import { AxiomStateStore } from "../src/state.js";

const balances = (): CreditBalanceSummary => ({format: "credits@3", scale: "1000000", revision: "2",
  purchased: {currency: "usd", scale: "100000000", posted_units: "9000000000000000", available_units: "9000000000000000", held_units: "0"},
  trial: {posted_units: "0", held_units: "0"}, allowance: {limit_units: "62500000", remaining_units: "62500000", held_units: "0", generation: null, resets_at: null, effective_until: null},
  subscription: null, paid_fallback: true, payment_review_required: false});

test("billing events preserve credit balances while account switches still clear them", () => {
  const store = new AxiomStateStore();
  store.setAccount({revision: 1, state: "valid", account: {id: "account-a", linkedMethods: []}});
  const status = {revision: 1, ledgerSequence: 2, currency: "microUSD" as const,
    postedMicrousd: 1_000_000, availableMicrousd: 1_000_000, paidMicrousd: 1_000_000, trialMicrousd: 0};
  store.setBilling(status);
  store.setCreditBalances(balances());
  store.extensionEvent({runtimeInstanceId: "billing-test", sequence: 1, occurredAt: "2026-11-13T12:00:00Z",
    event: {kind: "billing_changed", status: {...status, revision: 2, ledgerSequence: 3}}});
  assert.deepEqual(store.snapshot().billing?.credits, balances());
  store.extensionEvent({runtimeInstanceId: "billing-test", sequence: 2, occurredAt: "2026-11-13T12:00:01Z",
    event: {kind: "billing_changed", status}});
  assert.equal(store.snapshot().billing?.revision, 2);
  assert.deepEqual(store.snapshot().billing?.credits, balances());
  store.setAccount({revision: 2, state: "valid", account: {id: "account-b", linkedMethods: []}});
  assert.equal(store.snapshot().billing, null);
});

test("purchased credit is fixed USD and cannot be interpreted as allowance credits", () => {
  assert.equal(purchasedCreditAmount("1200900000"), "$12.00");
  assert.equal(purchasedCreditAmount("1200900000", true), "$12.009");
  assert.equal(inputPurchasedCredit("100.25"), "10025000000");
  assert.equal(purchasedCreditAmount("1"), "Less than $0.01");
  assert.throws(() => validateCreditBalances({...balances(), purchased: {...balances().purchased, scale: "1000000"}}));
  assert.throws(() => inputPurchasedCredit("1e2"));
});

test("credit summaries reject inconsistent buckets and preserve exact fractional balances", () => {
  validateCreditBalances(balances());
  assert.equal(creditAmount("9000000000000000", true), "9,000,000,000");
  assert.equal(creditAmount("1", true), "0.000001");
  assert.equal(creditAmount("1"), "Less than 1");
  for (const bad of ["-0", "01", "1e6", "9000000000000001"]) assert.throws(() => creditAmount(bad));
  const invalid = balances(); invalid.purchased.available_units = "1";
  assert.throws(() => validateCreditBalances(invalid));
  const negative = balances(); negative.allowance.remaining_units = "-1";
  assert.throws(() => validateCreditBalances(negative));
  const invalidTrial = balances(); invalidTrial.trial.held_units = "1";
  assert.throws(() => validateCreditBalances(invalidTrial));
});

test("billing actions are finite and cannot redirect native credentials or payment URLs", () => {
  validateCreditAction({kind: "usage", period: "month", timezone: "America/Los_Angeles"});
  validateCreditAction({kind: "checkout", id: crypto.randomUUID(), plan_key: "max", catalog_version: "plans-v1"});
  assert.throws(() => validateCreditAction({kind: "checkout", id: crypto.randomUUID(), plan_key: "max"}));
  for (const action of [{kind: "summary", account_id: "other"}, {kind: "post", url: "https://evil.invalid"},
    {kind: "checkout", id: "bad", plan_key: "max"}, {kind: "create_card_payment", id: crypto.randomUUID(), amount_microusd: 5_000_001}]) {
    assert.throws(() => validateCreditAction(action));
  }
  assert(allowedBillingUrl("https://checkout.stripe.com/c/pay/fixture"));
  assert(allowedBillingUrl("https://billing.stripe.com/p/session/fixture"));
  for (const url of ["https://evil.invalid", "https://checkout.stripe.com.evil.invalid/c/pay/x", "https://user@checkout.stripe.com/c/pay/x", "https://checkout.stripe.com/other", "javascript:alert(1)"]) assert(!allowedBillingUrl(url));
});

test("credit usage validates both bucket and model totals without changing its denomination", () => {
  const value = {period: "month", scale: "1000000", total_units: "1000001", allocation: {subscription_units: "1000000", trial_units: "1"}, purchased: {currency: "usd" as const, scale: "100000000" as const, total_units: "1000000", models: [{provider: "near", model_id: "model", units: "1000000"}]}, models: [{provider: "near", model_id: "model", units: "1000001"}]};
  assert.equal(creditUsageBreakdown(value, "month").total, 1000001n);
  assert.equal(purchasedUsageBreakdown(value, "month").total, 1000000n);
  assert.throws(() => purchasedUsageBreakdown({...value, purchased: {...value.purchased, currency: "zec"}}, "month"));
  assert.throws(() => creditUsageBreakdown({...value, allocation: {...value.allocation, trial_units: "0"}}, "month"));
  assert.throws(() => creditUsageBreakdown(value, "week"));
});

test("credit results negotiated with billing v7 cannot cross an account switch", async t => {
  const client = new AxiomAcpClient({command: process.execPath, args: ["-e", "process.stdin.resume()"]});
  t.after(() => client.close());
  let resolve!: (value: unknown) => void;
  t.mock.method(client.process, "request", async (method: string) => {
    if (method === "initialize") return {protocolVersion: 1, agentCapabilities: {_meta: {axiom: {protocolVersion: "0.2", runtimeInstanceId: "credits-test", features: {
      desktopChat: 1, threadCatalog: 1, timeline: 2, modelCatalog: 1, profilePreferences: 1, collections: 1, account: 2, billing: 7, securityEvidence: 4, webConsent: 1,
    }}}}};
    assert.equal(method, "_axiom/billing/credits");
    return new Promise(r => {resolve = r;});
  });
  await client.initialize();
  client.state.setAccount({revision: 1, state: "valid", account: {id: "account-a", linkedMethods: []}});
  const pending = client.creditBilling({kind: "summary"});
  await new Promise(r => setImmediate(r));
  const rejection = assert.rejects(pending, /account|context/i);
  client.state.setAccount({revision: 2, state: "valid", account: {id: "account-b", linkedMethods: []}});
  resolve({result: {kind: "summary", value: balances()}});
  await rejection;
  assert.equal(client.getState().billing, null);
});

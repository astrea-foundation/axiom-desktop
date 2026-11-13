import assert from "node:assert/strict";
import test from "node:test";

import {
  PRODUCTION_API_ORIGIN,
  PRODUCTION_AUTH_ORIGIN,
  STAGING_API_ORIGIN,
  STAGING_AUTH_ORIGIN,
  resolveServiceOrigins,
  activeInvoicePaymentUrl,
  requirePaymentAccount,
  trustedInvoicePaymentUrl,
  trustedNativeAuthorizationUrl,
} from "../src/main/service-origins";

test("unpackaged and packaged Desktop default to one production audience", () => {
  const expected = { auth: PRODUCTION_AUTH_ORIGIN, api: PRODUCTION_API_ORIGIN };
  assert.deepEqual(resolveServiceOrigins({ packaged: false }), expected);
  assert.deepEqual(
    resolveServiceOrigins({
      packaged: true,
      authUrl: "http://127.0.0.1:4174",
      apiUrl: "http://127.0.0.1:8080",
    }),
    expected,
  );
});

test("staging is an explicit coherent development audience, never a packaged override", () => {
  const staging = { authUrl: STAGING_AUTH_ORIGIN, apiUrl: STAGING_API_ORIGIN };
  assert.deepEqual(resolveServiceOrigins({ packaged: false, ...staging }), {
    auth: STAGING_AUTH_ORIGIN, api: STAGING_API_ORIGIN,
  });
  assert.deepEqual(resolveServiceOrigins({ packaged: true, ...staging }), {
    auth: PRODUCTION_AUTH_ORIGIN, api: PRODUCTION_API_ORIGIN,
  });
  assert.throws(() => resolveServiceOrigins({ packaged: false, authUrl: STAGING_AUTH_ORIGIN }));
  assert.throws(() => resolveServiceOrigins({ packaged: false, apiUrl: STAGING_API_ORIGIN }));
});

test("automatic checkout must match the active account's unexpired pending invoice", () => {
  const url = "https://shieldz.cash/pay/shieldz_invoice_1";
  const state: Parameters<typeof activeInvoicePaymentUrl>[1] = {
    connected: true,
    account: { revision: 1, state: "valid", account: { id: "account-a", linkedMethods: [] } },
    invoice: {
      revision: 1, id: "a".repeat(32), amountUsdCents: 2_500, state: "pending",
      payUrl: url, expiresAt: "2030-01-01T00:00:00.000Z",
    },
  };
  const now = Date.parse("2029-01-01T00:00:00.000Z");
  const validate = (value: string, current: typeof state) => activeInvoicePaymentUrl(value, current, "account-a", now);
  assert.equal(validate(url, state), url);
  assert.throws(() => validate(url, { ...state, connected: false }));
  assert.throws(() => validate(url, { ...state, account: null }));
  assert.throws(() => validate(url, { ...state, account: { revision: 2, state: "expired" } }));
  assert.throws(() => validate(url, { ...state, invoice: null }));
  assert.throws(() => validate(url + "_different", state));
  assert.throws(() => activeInvoicePaymentUrl(url, state, "account-b", now), /account changed/);
  assert.throws(() => requirePaymentAccount("account-b", state), /account changed/);
  assert.throws(() => requirePaymentAccount("", state), /account changed/);
  for (const status of ["paid", "expired", "failed", "review"] as const) {
    assert.throws(() => validate(url, { ...state, invoice: { ...state.invoice!, state: status } }));
  }
  for (const expiresAt of [null, "invalid", "2029-01-01T00:00:00.000Z"]) {
    assert.throws(() => validate(url, { ...state, invoice: { ...state.invoice!, expiresAt } }));
  }
  const foreignUrl = "https://evil.example/pay/shieldz_invoice_1";
  assert.throws(() => validate(foreignUrl, { ...state, invoice: { ...state.invoice!, payUrl: foreignUrl } }));
});

test("invoice checkout accepts only the bounded Shieldz payment capability", () => {
  const valid = "https://shieldz.cash/pay/shieldz_invoice_1";
  assert.equal(trustedInvoicePaymentUrl(valid), valid);
  for (const invalid of [
    "http://shieldz.cash/pay/shieldz_invoice_1",
    "https://evil.example/pay/shieldz_invoice_1",
    "https://user@shieldz.cash/pay/shieldz_invoice_1",
    "https://shieldz.cash/other/shieldz_invoice_1",
    "https://shieldz.cash/pay/shieldz_invoice_1?next=https://evil.example",
    "https://shieldz.cash/pay/shieldz_invoice_1#fragment",
    "https://shieldz.cash/pay/../account",
  ]) {
    assert.throws(() => trustedInvoicePaymentUrl(invalid));
  }
});

test("development overrides must form a coherent bounded deployment pair", () => {
  assert.deepEqual(
    resolveServiceOrigins({
      packaged: false,
      authUrl: "http://127.0.0.1:4174",
      apiUrl: "https://localhost:8080",
    }),
    { auth: "http://127.0.0.1:4174", api: "https://localhost:8080" },
  );
  assert.deepEqual(
    resolveServiceOrigins({
      packaged: false,
      authUrl: "https://localhost:4174",
      apiUrl: "https://127.0.0.1:8080",
    }),
    { auth: "https://localhost:4174", api: "https://127.0.0.1:8080" },
  );
  assert.throws(() => resolveServiceOrigins({
    packaged: false,
    authUrl: "http://localhost:4174",
    apiUrl: "http://localhost:8080",
  }), /HTTPS/);
  assert.throws(() =>
    resolveServiceOrigins({ packaged: false, authUrl: "http://127.0.0.1:4174" }),
  );
  for (const authUrl of [
    "https://evil.example",
    "https://auth.axiom.stream/other",
    "https://user@auth.axiom.stream",
  ]) {
    assert.throws(() => resolveServiceOrigins({ packaged: false, authUrl }));
  }
});

test("native authorization URLs require the exact resolved auth origin and route", () => {
  const valid = `${PRODUCTION_AUTH_ORIGIN}/native/authorize?authorization_id=${"a".repeat(32)}&state=${"s".repeat(64)}`;
  assert.equal(trustedNativeAuthorizationUrl(valid, PRODUCTION_AUTH_ORIGIN), valid);
  assert.throws(() => trustedNativeAuthorizationUrl(valid.replace("auth.", "evil."), PRODUCTION_AUTH_ORIGIN));
  assert.throws(() => trustedNativeAuthorizationUrl(`${PRODUCTION_AUTH_ORIGIN}/account`, PRODUCTION_AUTH_ORIGIN));
  assert.throws(() => trustedNativeAuthorizationUrl(`${valid}#fragment`, PRODUCTION_AUTH_ORIGIN));
  assert.throws(() => trustedNativeAuthorizationUrl(`${valid}&state=${"x".repeat(64)}`, PRODUCTION_AUTH_ORIGIN));
  assert.throws(() => trustedNativeAuthorizationUrl(`${valid}&next=https://evil.example`, PRODUCTION_AUTH_ORIGIN));
});

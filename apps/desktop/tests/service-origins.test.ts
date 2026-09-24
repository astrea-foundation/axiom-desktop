import assert from "node:assert/strict";
import test from "node:test";

import {
  PRODUCTION_API_ORIGIN,
  PRODUCTION_AUTH_ORIGIN,
  STAGING_API_ORIGIN,
  STAGING_AUTH_ORIGIN,
  resolveServiceOrigins,
  requirePaymentAccount,
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

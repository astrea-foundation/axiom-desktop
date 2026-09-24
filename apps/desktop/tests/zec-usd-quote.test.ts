import assert from "node:assert/strict";
import test from "node:test";
import { currentZecUsdQuote } from "../src/renderer/src/zecUsdQuote.js";

test("current quotes expire at their market timestamp even if polling stops", () => {
  const now = Date.parse("2026-09-09T12:00:00Z");
  const quote = { source: "coinbase", price_microusd_per_zec: "123456789",
    as_of: new Date(now).toISOString(), expires_at: new Date(now + 60_000).toISOString() };
  assert.equal(currentZecUsdQuote(quote, now), quote);
  assert.equal(currentZecUsdQuote(quote, now + 59_999), quote);
  assert.equal(currentZecUsdQuote(quote, now + 60_000), null);
  assert.equal(currentZecUsdQuote(quote, now - 1), null);
  for (const value of [null, undefined, { ...quote, source: "unknown" },
    { ...quote, as_of: "invalid" }, { ...quote, expires_at: new Date(now + 60_001).toISOString() },
    ...["0", "01", "1.5", "-1", "NaN", "1000000000001"].map(price_microusd_per_zec => ({ ...quote, price_microusd_per_zec }))]) {
    assert.equal(currentZecUsdQuote(value, now), null);
  }
});

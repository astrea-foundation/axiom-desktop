import assert from "node:assert/strict";
import test from "node:test";
import { formatModelPricing } from "../src/renderer/src/components/modelPricing.js";

test("model picker formats exact NEAR and Tinfoil prices per million tokens", () => {
  assert.equal(formatModelPricing({
    inputPriceMicrousdPerMillionTokens: 440_000,
    outputPriceMicrousdPerMillionTokens: 1_320_000,
  }), "$0.44 in, $1.32 out");
  assert.equal(formatModelPricing({
    inputPriceMicrousdPerMillionTokens: 50_000_000,
    outputPriceMicrousdPerMillionTokens: 125_000,
  }), "$50 in, $0.125 out");
  assert.equal(formatModelPricing({
    inputPriceMicrousdPerMillionTokens: null,
    outputPriceMicrousdPerMillionTokens: null,
  }), null);
});

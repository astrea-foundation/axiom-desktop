import assert from "node:assert/strict";
import test from "node:test";
import type { UsageSummary } from "@axiom/axiom-acp-client";
import { formatSpend, shareLabel, spendingBreakdown } from "../src/renderer/src/usageSpend";

const model = (costMicrousd: string, modelId = "a", provider = "test") => ({ provider, modelId, modelName: modelId, costMicrousd });
const summary = (totalCostMicrousd: string, models: UsageSummary["models"]): UsageSummary => ({ period: "all_time", totalCostMicrousd, models });

test("spending merges the same model across sources and keeps providers distinct", () => {
  const result = spendingBreakdown(summary("1000000", [model("200000"), model("300000"), model("400000", "b"), model("100000", "a", "other")]));
  assert.equal(result.total, 1000000n);
  assert.deepEqual(result.models.map(row => [row.provider, row.label, row.amount, shareLabel(row)]), [
    ["test", "a", 500000n, "50%"], ["test", "b", 400000n, "40%"], ["other", "a", 100000n, "10%"],
  ]);
});

test("spending retains microdollar precision above the JS safe integer limit", () => {
  const cost = "9007199254740993";
  assert.equal(spendingBreakdown(summary(cost, [model(cost)])).total, 9007199254740993n);
  assert.equal(formatSpend(9007199254740993n), "$9,007,199,254.740993");
  assert.equal(formatSpend(1n), "$0.000001");
  assert.equal(formatSpend(1200000n), "$1.20");
});

test("empty spending has no slices and missing model identity fails without hiding charges", () => {
  assert.deepEqual(spendingBreakdown(summary("0", [])).models, []);
  assert.equal(formatSpend(0n), "$0.00");
  for (const modelName of [null, "A display name"]) {
    assert.throws(() => spendingBreakdown(summary("1", [{ provider: "test", modelId: null, modelName, costMicrousd: "1" }])));
  }
  const result = spendingBreakdown(summary("1", [{ provider: "test", modelId: "model-a", modelName: null, costMicrousd: "1" }]));
  assert.equal(result.models[0]?.label, "model-a");
});

test("invalid amounts and inconsistent totals fail instead of displaying a misleading chart", () => {
  for (const cost of ["-1", "01", "1.5", "1e6", "", "NaN"]) {
    assert.throws(() => spendingBreakdown(summary(cost, [model(cost)])));
  }
  assert.throws(() => spendingBreakdown(summary("200", [model("100")])));
  assert.throws(() => spendingBreakdown({ ...summary("0", []), period: "month" }));
  assert.throws(() => spendingBreakdown(summary("1", [model("1", "\u0000bad")])));
});


test("spending accepts only totals for the selected period", () => {
  for (const period of ["week", "month", "all_time"] as const) {
    const value = { ...summary("1234567", [model("1234567")]), period };
    assert.equal(spendingBreakdown(value, period).total, 1234567n);
    for (const different of ["week", "month", "all_time"] as const) {
      if (different !== period) assert.throws(() => spendingBreakdown(value, different));
    }
  }
});

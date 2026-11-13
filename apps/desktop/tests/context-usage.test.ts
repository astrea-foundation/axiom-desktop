import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ContextUsageIndicator, contextUsagePresentation, requestUsageTotals } from "../src/renderer/src/components/ContextUsageIndicator";

const sample = {
  usage: {
    inputTokens: 75_000, outputTokens: 1_000, modelId: "test-model",
    reportedAt: "2026-09-07T00:00:00Z",
    contextWindowTokens: 100_000, autoCompactThresholdTokens: 85_000,
  },
  selectedModelId: "test-model",
  contextWindowTokens: 100_000, autoCompactThresholdTokens: 85_000,
};

test("request accounting uses exact unique cumulative counters, subsets, and nullable elapsed time", () => {
  const record = {
    requestId: "a".repeat(32), modelId: "m", providerId: "near",
    purpose: "conversation", state: "completed", completeness: "final",
    inputTokens: "9007199254740993", cachedInputTokens: "10", outputTokens: "20",
    reasoningTokens: "15", costMicrousd: "1000001", settled: true, responseVerified: true,
    startedAtMs: "1000", finishedAtMs: "6000",
  } as const;
  const result = requestUsageTotals([record, record, { ...record, inputTokens: "9007199254740994" }]);
  assert.equal(result.records.length, 1);
  assert.equal(result.sum("inputTokens"), 9007199254740994n);
  assert.equal(result.sum("outputTokens"), 20n);
  assert.equal(result.sum("reasoningTokens"), 15n);
  assert.equal(result.cost, 1000001n);
  assert.equal(result.elapsed, 5000);
  assert.equal(result.pending, false);
  const unresolved = requestUsageTotals([{ ...record, state: "failed", inputTokens: null, finishedAtMs: null, costMicrousd: null, settled: false }]);
  assert.equal(unresolved.elapsed, null);
  assert.equal(unresolved.pending, true);
  assert.equal(unresolved.unknown, true);
});

test("context meter shows last provider-reported input plus output and pinned limits", () => {
  const view = contextUsagePresentation(sample);
  assert.equal(view.percentage, 76);
  assert.equal(view.usageLabel, "76,000 tokens");
  assert.equal(view.breakdownLabel, "75,000 input, 1,000 output");
  assert.equal(view.capacityLabel, "100,000 token context window");
  assert.equal(view.compactionLabel, "Auto-compacts at 85,000 tokens (85%)");
  const markup = renderToStaticMarkup(createElement(ContextUsageIndicator, sample));
  assert.match(markup, /aria-label="Usage"/);
  assert.match(markup, /aria-valuenow="76"/);
  assert.match(markup, /stroke-dasharray="76 100"/);
  assert.ok(markup.includes(">76</span>"));
  assert.doesNotMatch(markup, /About |Estimated/);
});

test("model switches do not relabel or scale a historical report", () => {
  const view = contextUsagePresentation({ ...sample, selectedModelId: "another-model", contextWindowTokens: 1_000_000, autoCompactThresholdTokens: 262_144 });
  assert.equal(view.percentageLabel, "76% used");
  assert.equal(view.capacityLabel, "100,000 token context window");
  assert.equal(view.compactionLabel, "Auto-compacts at 85,000 tokens (85%)");
  const updated = contextUsagePresentation({ ...sample, usage: { ...sample.usage, inputTokens: 300_000, outputTokens: 5_000, contextWindowTokens: 1_000_000, autoCompactThresholdTokens: 262_144 } });
  assert.equal(updated.percentageLabel, "30.5% used");
  assert.equal(updated.compactionLabel, "Auto-compacts at 262,144 tokens (26.2%)");
});

test("missing, malformed, and legacy estimated usage never becomes a plausible report", () => {
  assert.equal(contextUsagePresentation({ ...sample, usage: null }).percentage, null);
  assert.equal(contextUsagePresentation({ ...sample, usage: null }).usageLabel, "No usage reported yet");
  const legacy = { usedTokens: 10_000, estimated: false } as unknown as typeof sample.usage;
  assert.equal(contextUsagePresentation({ ...sample, usage: legacy }).percentage, null);
  for (const capacity of [undefined, 0, -1, NaN, Infinity]) {
    assert.equal(contextUsagePresentation({ ...sample, usage: { ...sample.usage, contextWindowTokens: capacity } }).percentage, null);
  }
  for (const inputTokens of [-1, NaN, Infinity, Number.MAX_SAFE_INTEGER]) {
    assert.equal(contextUsagePresentation({ ...sample, usage: { ...sample.usage, inputTokens } }).percentage, null);
  }
  assert.equal(contextUsagePresentation({ ...sample, usage: { ...sample.usage, autoCompactThresholdTokens: 200_000 } }).compactionLabel, "Compaction threshold unavailable");
});

test("zero reported usage and over-capacity reports remain honest", () => {
  assert.equal(contextUsagePresentation({ ...sample, usage: { ...sample.usage, inputTokens: 0, outputTokens: 0 } }).percentageLabel, "0% used");
  const props = { ...sample, usage: { ...sample.usage, inputTokens: 119_000 } };
  assert.equal(contextUsagePresentation(props).percentageLabel, "120% used");
  const markup = renderToStaticMarkup(createElement(ContextUsageIndicator, props));
  assert.match(markup, /stroke-dasharray="100 100"/);
  assert.match(markup, /aria-valuenow="100"/);
  assert.match(markup, /120% used/);
});

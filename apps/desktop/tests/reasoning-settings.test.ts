import assert from "node:assert/strict";
import test from "node:test";
import { explicitReasoningEfforts, reconcileReasoningEffort } from "../src/renderer/src/reasoningSettings";
import type { ReasoningEffort } from "../src/renderer/src/types";

test("old preferences resolve only to available thinking controls", () => {
  const binary: ReasoningEffort[] = ["provider_default", "enabled", "disabled"];
  assert.equal(reconcileReasoningEffort("medium", binary), "enabled");
  assert.equal(reconcileReasoningEffort("disabled", binary), "disabled");
  assert.equal(reconcileReasoningEffort("provider_default", binary), "enabled");
  assert.equal(reconcileReasoningEffort("enabled", ["provider_default", "disabled"]), "disabled");
  assert.equal(reconcileReasoningEffort("enabled", ["disabled"]), "disabled");
  assert.equal(reconcileReasoningEffort("xhigh", ["low", "medium"]), "medium");
  assert.equal(reconcileReasoningEffort("medium", ["low"]), "low");
  assert.equal(reconcileReasoningEffort("high", []), "provider_default");
  assert.deepEqual(explicitReasoningEfforts(binary), ["enabled", "disabled"]);
  assert.deepEqual(explicitReasoningEfforts(["provider_default"]), []);
  assert.equal(reconcileReasoningEffort("provider_default", ["low", "medium"]), "medium");
});

test("capability changes never retain a selection outside the advertised list", () => {
  const all: ReasoningEffort[] = ["provider_default", "enabled", "disabled", "minimal", "low", "medium", "high", "xhigh"];
  for (const options of [all, ["disabled"], ["high", "xhigh"], ["provider_default", "enabled"]] as ReasoningEffort[][]) {
    for (const previous of [...all, "future-mode", undefined]) {
      const next = reconcileReasoningEffort(previous, options);
      assert.ok(options.includes(next));
      assert.notEqual(next, "provider_default");
      if (previous !== "provider_default" && options.includes(previous as ReasoningEffort)) assert.equal(next, previous);
    }
  }
});

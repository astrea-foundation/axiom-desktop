import assert from "node:assert/strict";
import test from "node:test";
import type { ProviderModel } from "@axiom/chat-ui";
import { compareModelPreference } from "@axiom/chat-ui/modelPreferences";

const model = (id: string, providerId: string, upstream: string): ProviderModel => ({
  id, label: id, shortLabel: id, providerId, providerLabel: providerId, model: upstream,
  supportedReasoningEfforts: [], inputPriceMicrousdPerMillionTokens: null, outputPriceMicrousdPerMillionTokens: null,
});

const catalog = [
  model("glm-5-3-flash", "near", "z-ai/glm-5.3-flash"),
  model("tinfoil-kimi-k3-1-pro", "tinfoil", "kimi-k3-1-pro"),
  model("tinfoil-deepseek-v4-1-flash", "tinfoil", "deepseek-v4-1-flash"),
  model("deepseek-v4-1-flash-1017-abliterated", "near", "deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated"),
  model("direct:qwen3.8-27b", "axiom-direct", "Qwen3.8-27B-Uncensored"),
];
const order = (models: ProviderModel[]) => [...models].sort(compareModelPreference).map((entry) => entry.id);

test("NEAR's abliterated DeepSeek is the default, with Tinfoil's DeepSeek as the fallback", () => {
  assert.deepEqual(order(catalog), [
    "deepseek-v4-1-flash-1017-abliterated",
    "tinfoil-deepseek-v4-1-flash",
    "tinfoil-kimi-k3-1-pro",
    "glm-5-3-flash",
    "direct:qwen3.8-27b",
  ]);
  assert.equal(order(catalog.filter((entry) => entry.providerId !== "near" || !entry.model.includes("abliterated")))[0], "tinfoil-deepseek-v4-1-flash");
  // The same upstream name from another provider is not the default.
  assert.equal(order([...catalog.slice(0, 3), model("other", "other", "deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated")])[0], "tinfoil-deepseek-v4-1-flash");
});

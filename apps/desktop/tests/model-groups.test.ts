import assert from "node:assert/strict";
import test from "node:test";
import type { ProviderModel } from "@axiom/chat-ui";
import { averagePrice, compareModelGroups, groupModels, modelGroupKey, preferredOffering } from "@axiom/chat-ui/modelGroups";

const offering = (
  id: string,
  providerId: string,
  upstream: string,
  prices: [number, number] | null,
  capabilities: { images?: boolean; files?: boolean } = {},
  label = upstream,
): ProviderModel => ({
  id,
  label,
  shortLabel: label,
  providerId,
  providerLabel: providerId,
  model: upstream,
  supportedReasoningEfforts: [],
  inputPriceMicrousdPerMillionTokens: prices?.[0] ?? null,
  outputPriceMicrousdPerMillionTokens: prices?.[1] ?? null,
  supportsImages: capabilities.images ?? false,
  fileMimeTypes: capabilities.files ? ["application/pdf"] : [],
});

const ids = (models: readonly ProviderModel[]) => models.map((model) => model.id);

test("the same upstream model from different providers is one group", () => {
  assert.equal(modelGroupKey(offering("a", "near", "deepseek-ai/DeepSeek-V4.1-Flash", null)), "deepseek-v4-1-flash");
  assert.equal(modelGroupKey(offering("b", "tinfoil", "deepseek-v4-1-flash", null)), "deepseek-v4-1-flash");
  assert.equal(modelGroupKey(offering("c", "near", "z-ai/GLM-5.3-Flash", null)), "glm-5-3-flash");
  assert.equal(modelGroupKey(offering("fallback-id", "near", "///", null)), "fallback-id");
  const groups = groupModels([
    offering("tinfoil-deepseek", "tinfoil", "deepseek-v4-1-flash", [300_000, 1_200_000]),
    offering("glm", "near", "z-ai/glm-5.3", [900_000, 3_200_000]),
    offering("deepseek", "near", "deepseek-ai/DeepSeek-V4.1-Flash", [280_000, 1_100_000]),
  ]);
  assert.deepEqual(groups.map((group) => [group.key, ids(group.offerings)]), [
    ["deepseek-v4-1-flash", ["deepseek", "tinfoil-deepseek"]],
    ["glm-5-3", ["glm"]],
  ]);
});

test("more upload capabilities win before price", () => {
  const textOnly = offering("cheap-text", "near", "kimi-k3", [100_000, 100_000]);
  const images = offering("pricey-images", "tinfoil", "kimi-k3", [2_000_000, 8_000_000], { images: true });
  const both = offering("priciest-both", "axiom-direct", "kimi-k3", [5_000_000, 9_000_000], { images: true, files: true });
  assert.deepEqual(ids(groupModels([textOnly, images])[0]!.offerings), ["pricey-images", "cheap-text"]);
  assert.deepEqual(ids(groupModels([textOnly, images, both])[0]!.offerings), ["priciest-both", "pricey-images", "cheap-text"]);
});

test("the lowest average of input and output price wins, unpriced last", () => {
  // Averages are $1.00 and $0.90 per 1M tokens: a lower input price alone does not win.
  const lowInput = offering("low-input", "near", "glm-5-3", [100_000, 1_900_000]);
  const balanced = offering("balanced", "tinfoil", "glm-5-3", [800_000, 1_000_000]);
  const unpriced = offering("unpriced", "axiom-direct", "glm-5-3", null);
  assert.equal(averagePrice(lowInput), 1_000_000);
  assert.equal(averagePrice(unpriced), null);
  assert.deepEqual(ids(groupModels([unpriced, lowInput, balanced])[0]!.offerings), ["balanced", "low-input", "unpriced"]);
});

test("equal capabilities and average price always pick the same provider", () => {
  const near = offering("near-glm", "near", "glm-5-3", [500_000, 1_500_000]);
  const tinfoil = offering("tinfoil-glm", "tinfoil", "glm-5-3", [1_000_000, 1_000_000]);
  const direct = offering("direct-glm", "axiom-direct", "glm-5-3", [1_500_000, 500_000]);
  for (const order of [[near, tinfoil, direct], [direct, near, tinfoil], [tinfoil, direct, near]]) {
    assert.deepEqual(ids(groupModels(order)[0]!.offerings), ["direct-glm", "near-glm", "tinfoil-glm"]);
  }
});

test("a new thread resolves any offering of a model to its preferred provider", () => {
  const near = offering("deepseek", "near", "deepseek-ai/DeepSeek-V4.1-Flash", [280_000, 1_100_000], { files: true });
  const tinfoil = offering("tinfoil-deepseek", "tinfoil", "deepseek-v4-1-flash", [300_000, 1_200_000], { images: true, files: true });
  const other = offering("glm", "near", "z-ai/glm-5.3", [900_000, 3_200_000]);
  const models = [near, tinfoil, other];
  assert.equal(preferredOffering(models, "deepseek")?.id, "tinfoil-deepseek");
  assert.equal(preferredOffering(models, "tinfoil-deepseek")?.id, "tinfoil-deepseek");
  assert.equal(preferredOffering(models, "glm")?.id, "glm");
  assert.equal(preferredOffering(models, "missing"), undefined);
  // Attachments narrow the choice to providers that accept them.
  assert.equal(preferredOffering(models, "deepseek", { accepts: (model) => model.providerId === "near" })?.id, "deepseek");
  assert.equal(preferredOffering(models, "deepseek", { accepts: () => false }), undefined);
  // A remembered offering whose provider is down moves to an available provider of the same model.
  const down = offering("down-deepseek", "axiom-direct", "deepseek-v4-1-flash", [100_000, 100_000], { images: true, files: true });
  assert.equal(preferredOffering([near, other], "down-deepseek", { known: [down] })?.id, "deepseek");
  assert.equal(preferredOffering([other], "down-deepseek", { known: [down] }), undefined);
});

test("groups list the default models first, then by name", () => {
  const groups = groupModels([
    offering("tinfoil-glm", "tinfoil", "glm-5-3", null, {}, "GLM 5.3"),
    offering("tinfoil-deepseek", "tinfoil", "deepseek-v4-1-flash", null, {}, "DeepSeek V4.1 Flash"),
    offering("abliterated", "near", "deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated", null, {}, "DeepSeek V4.1 Flash Abliterated"),
    offering("gpt", "tinfoil", "gpt-oss-120b", null, {}, "gpt-oss-120b"),
    offering("glm-flash", "near", "z-ai/glm-5.3-flash", null, {}, "GLM 5.3 Flash"),
  ]).sort(compareModelGroups);
  assert.deepEqual(groups.map((group) => group.offerings[0]!.label), [
    "DeepSeek V4.1 Flash Abliterated",
    "DeepSeek V4.1 Flash",
    "GLM 5.3",
    "GLM 5.3 Flash",
    "gpt-oss-120b",
  ]);
});

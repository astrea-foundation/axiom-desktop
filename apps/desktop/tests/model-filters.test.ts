import assert from "node:assert/strict";
import test from "node:test";
import type { ModelTag, ProviderModel } from "@axiom/chat-ui";
import {
  catalogCapabilities,
  catalogHosts,
  catalogTags,
  countModels,
  emptyModelFilters,
  filterModels,
  hasActiveModelFilters,
  toggled,
  type ModelFilters,
} from "@axiom/chat-ui/modelFilters";

const tag = (id: string, label: string, position: number, tone: ModelTag["tone"] = "gray"): ModelTag => ({ id, label, tone, position });
const cheap = tag("cheap", "Cheap", 10, "green");
const smart = tag("intelligent", "Intelligent", 20, "violet");
const uncensored = tag("uncensored", "Uncensored", 50, "pink");
const fresh = tag("new", "New", 1000, "orange");

const model = (
  id: string,
  providerId: string,
  upstream: string,
  tags: ModelTag[] = [],
  capabilities: { images?: boolean; files?: boolean } = {},
): ProviderModel => ({
  id,
  label: id,
  shortLabel: id,
  providerId,
  providerLabel: providerId === "near" ? "NEAR AI" : providerId === "tinfoil" ? "Tinfoil" : "AstreaAI",
  model: upstream,
  supportedReasoningEfforts: [],
  inputPriceMicrousdPerMillionTokens: null,
  outputPriceMicrousdPerMillionTokens: null,
  supportsImages: capabilities.images ?? false,
  fileMimeTypes: capabilities.files ? ["application/pdf"] : [],
  tags,
});

const catalog = [
  model("glm-5-3-flash", "near", "z-ai/glm-5.3-flash", [cheap], { files: true }),
  model("qwen3-6-35b", "near", "Qwen/Qwen3-6-35B", [cheap], { images: true, files: true }),
  model("minimax-m3", "near", "MiniMaxAI/MiniMax-M3", [fresh], { files: true }),
  model("direct:qwen3.8-27b", "axiom-direct", "Qwen3.8-27B-Uncensored", [cheap, uncensored], { images: true }),
  model("tinfoil-kimi-k3", "tinfoil", "kimi-k3", [smart], { files: true }),
  model("tinfoil-deepseek-v4-1-flash", "tinfoil", "deepseek-v4-1-flash", [cheap], { files: true }),
];

const filters = (patch: Partial<ModelFilters> = {}): ModelFilters => ({ ...emptyModelFilters(), ...patch });
const ids = (models: ProviderModel[]) => models.map((entry) => entry.id);

test("tags combine with AND, hosts with OR, and results keep provider preference order", () => {
  assert.deepEqual(ids(filterModels(catalog, filters())), [
    "tinfoil-deepseek-v4-1-flash",
    "tinfoil-kimi-k3",
    "glm-5-3-flash",
    "minimax-m3",
    "qwen3-6-35b",
    "direct:qwen3.8-27b",
  ]);
  assert.deepEqual(ids(filterModels(catalog, filters({ tags: new Set(["cheap", "uncensored"]) }))), ["direct:qwen3.8-27b"]);
  assert.deepEqual(
    ids(filterModels(catalog, filters({ tags: new Set(["cheap"]), hosts: new Set(["tinfoil", "axiom-direct"]) }))),
    ["tinfoil-deepseek-v4-1-flash", "direct:qwen3.8-27b"],
  );
  assert.deepEqual(
    ids(filterModels(catalog, filters({ tags: new Set(["cheap"]), capabilities: new Set(["images"]) }))),
    ["qwen3-6-35b", "direct:qwen3.8-27b"],
  );
});

test("search matches names, upstream models, hosts and tag labels", () => {
  assert.deepEqual(ids(filterModels(catalog, filters({ query: "uncensored" }))), ["direct:qwen3.8-27b"]);
  assert.deepEqual(ids(filterModels(catalog, filters({ query: "minimaxai" }))), ["minimax-m3"]);
  assert.deepEqual(ids(filterModels(catalog, filters({ query: "tinfoil" }))), ["tinfoil-deepseek-v4-1-flash", "tinfoil-kimi-k3"]);
  assert.equal(filterModels(catalog, filters({ query: "nothing-like-this" })).length, 0);
});

test("filter options come from the catalog and counts preview the next selection", () => {
  assert.deepEqual(catalogTags(catalog).map((entry) => entry.id), ["cheap", "intelligent", "uncensored", "new"]);
  assert.deepEqual(catalogHosts(catalog), [
    { id: "tinfoil", label: "Tinfoil" },
    { id: "near", label: "NEAR AI" },
    { id: "axiom-direct", label: "AstreaAI" },
  ]);
  assert.deepEqual(catalogCapabilities(catalog), ["images", "files"]);
  assert.deepEqual(catalogCapabilities(catalog.filter((entry) => entry.supportsImages)), ["files"]);
  assert.deepEqual(catalogTags([model("legacy", "near", "legacy")]), []);

  const cheapOnly = filters({ tags: new Set(["cheap"]) });
  assert.equal(countModels(catalog, cheapOnly), 4);
  assert.equal(countModels(catalog, cheapOnly, { tags: new Set(["cheap", "intelligent"]) }), 0);
  assert.equal(countModels(catalog, cheapOnly, { hosts: new Set(["near"]) }), 2);
  assert.equal(countModels(catalog, cheapOnly, { hosts: new Set() }), 4);
});

test("toggling and clearing filters is immutable", () => {
  const original = new Set(["cheap"]);
  const added = toggled(original, "new");
  assert.deepEqual([...added], ["cheap", "new"]);
  assert.deepEqual([...toggled(added, "cheap")], ["new"]);
  assert.deepEqual([...original], ["cheap"]);
  assert.equal(hasActiveModelFilters(filters()), false);
  assert.equal(hasActiveModelFilters(filters({ hosts: new Set(["near"]) })), true);
  assert.equal(hasActiveModelFilters(filters({ query: "glm" })), true);
});

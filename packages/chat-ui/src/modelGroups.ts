import type { ProviderModel } from "./types";
import { defaultModelRank } from "./modelPreferences";

/**
 * One model as people see it, served by one or more providers. Grouping is
 * presentation and provider choice only: every request still names one exact
 * catalog offering, which keeps its own attestation and E2EE checks.
 */
export interface ModelGroup {
  key: string;
  /** Best offering first, by {@link compareOfferings}. */
  offerings: ProviderModel[];
}

/**
 * Provider-neutral model name, e.g. `z-ai/GLM-5.3-Flash` → `glm-5-3-flash`.
 * Same normalization the platform uses to share curated tags between hosts.
 */
export function modelGroupKey(model: Pick<ProviderModel, "id" | "model">): string {
  // Entries without an upstream name stay on their own under their offering ID.
  const name = typeof model.model === "string" ? model.model.split("/").pop() ?? "" : "";
  return name.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-+|-+$/g, "") || model.id;
}

/** Image and file uploads the offering accepts. */
export function capabilityCount(model: ProviderModel): number {
  return Number(model.supportsImages === true) + Number((model.fileMimeTypes?.length ?? 0) > 0);
}

/** Mean of the input and output prices per million tokens; null when unpriced. */
export function averagePrice(model: ProviderModel): number | null {
  const input = model.inputPriceMicrousdPerMillionTokens;
  const output = model.outputPriceMicrousdPerMillionTokens;
  return input == null || output == null ? null : (input + output) / 2;
}

const byCodePoint = (left: string, right: string): number => (left < right ? -1 : left > right ? 1 : 0);

/**
 * Which provider serves a model: the one accepting more kinds of uploads, then
 * the lowest average price, then provider ID so ties always resolve the same way.
 */
export function compareOfferings(left: ProviderModel, right: ProviderModel): number {
  const leftPrice = averagePrice(left);
  const rightPrice = averagePrice(right);
  return capabilityCount(right) - capabilityCount(left)
    || (leftPrice === null ? 1 : 0) - (rightPrice === null ? 1 : 0)
    || (leftPrice !== null && rightPrice !== null ? leftPrice - rightPrice : 0)
    || byCodePoint(left.providerId, right.providerId)
    || byCodePoint(left.id, right.id);
}

/** Groups in the order their first offering appears; offerings best first. */
export function groupModels(models: readonly ProviderModel[]): ModelGroup[] {
  const groups = new Map<string, ProviderModel[]>();
  for (const model of models) {
    const key = modelGroupKey(model);
    const offerings = groups.get(key);
    if (offerings) offerings.push(model);
    else groups.set(key, [model]);
  }
  return [...groups].map(([key, offerings]) => ({ key, offerings: offerings.sort(compareOfferings) }));
}

/** The default model's group first, then the fallback default's, then by name. */
export function compareModelGroups(left: ModelGroup, right: ModelGroup): number {
  const rank = (group: ModelGroup) => Math.min(...group.offerings.map(defaultModelRank));
  const leftLabel = left.offerings[0]!.label;
  const rightLabel = right.offerings[0]!.label;
  return rank(left) - rank(right)
    || leftLabel.localeCompare(rightLabel, undefined, { sensitivity: "base", numeric: true })
    || byCodePoint(left.key, right.key);
}

/**
 * The offering a new thread should use for the model behind `modelId`: the
 * best available provider for that model that also satisfies `accepts` (for
 * example, the draft's attachments). `known` may add display-only entries,
 * such as offerings whose provider is down, so their model can still be found.
 * Returns undefined when `modelId` is unknown or no provider qualifies.
 */
export function preferredOffering(
  models: readonly ProviderModel[],
  modelId: string,
  options: { known?: readonly ProviderModel[]; accepts?: (model: ProviderModel) => boolean } = {},
): ProviderModel | undefined {
  const chosen = models.find((model) => model.id === modelId)
    ?? options.known?.find((model) => model.id === modelId);
  if (!chosen) return undefined;
  const key = modelGroupKey(chosen);
  return models
    .filter((model) => modelGroupKey(model) === key && (options.accepts?.(model) ?? true))
    .sort(compareOfferings)[0];
}

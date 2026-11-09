import type { ProviderModel } from "./types";

/** NEAR's attested, abliterated DeepSeek V4.1 Flash is the default; Tinfoil's DeepSeek V4.1 Flash is the fallback. */
const DEFAULT_MODEL = { providerId: "near", model: "deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated" } as const;
const FALLBACK_DEFAULT_MODEL = { providerId: "tinfoil", model: "deepseek-v4-1-flash" } as const;

const pinnedRank = (model: ProviderModel): number => (
  model.providerId === DEFAULT_MODEL.providerId && model.model === DEFAULT_MODEL.model ? 0
    : model.providerId === FALLBACK_DEFAULT_MODEL.providerId && model.model === FALLBACK_DEFAULT_MODEL.model ? 1
      : 2
);

const providerPriority = (provider: string): number => provider === "tinfoil" ? 0 : provider === "near" ? 1 : 2;

/** Presentation/default preference only; every entry still comes from the native catalog. */
export function compareModelPreference(left: ProviderModel, right: ProviderModel): number {
  return pinnedRank(left) - pinnedRank(right)
    || providerPriority(left.providerId) - providerPriority(right.providerId)
    || left.providerId.localeCompare(right.providerId)
    || left.id.localeCompare(right.id);
}

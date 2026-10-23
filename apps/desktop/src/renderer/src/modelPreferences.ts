import type { ProviderModel } from "./types";

const providerPriority = (provider: string): number => provider === "tinfoil" ? 0 : provider === "near" ? 1 : 2;

/** Presentation/default preference only; every entry still comes from the native catalog. */
export function compareModelPreference(left: ProviderModel, right: ProviderModel): number {
  return providerPriority(left.providerId) - providerPriority(right.providerId)
    || left.providerId.localeCompare(right.providerId)
    || Number(right.providerId === "tinfoil" && right.model === "deepseek-v4-1-flash")
      - Number(left.providerId === "tinfoil" && left.model === "deepseek-v4-1-flash")
    || left.id.localeCompare(right.id);
}

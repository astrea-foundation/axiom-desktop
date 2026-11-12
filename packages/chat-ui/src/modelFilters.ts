import type { ModelTag, ProviderModel } from "./types";
import { compareModelPreference } from "./modelPreferences";

/** Filters derived from fields every catalog entry already carries. */
export type ModelCapability = "images" | "files";

/**
 * Model picker filters. Tags combine with AND, hosts with OR, and
 * capabilities with AND. Filtering is display-only: it never changes which
 * models the catalog authorizes.
 */
export interface ModelFilters {
  /** Trimmed, lower-cased search text. */
  query: string;
  tags: ReadonlySet<string>;
  hosts: ReadonlySet<string>;
  capabilities: ReadonlySet<ModelCapability>;
}

export interface HostOption {
  id: string;
  label: string;
}

export function emptyModelFilters(): ModelFilters {
  return { query: "", tags: new Set(), hosts: new Set(), capabilities: new Set() };
}

export function hasActiveModelFilters(filters: ModelFilters): boolean {
  return filters.query.length > 0
    || filters.tags.size > 0
    || filters.hosts.size > 0
    || filters.capabilities.size > 0;
}

export function modelHasCapability(model: ProviderModel, capability: ModelCapability): boolean {
  return capability === "images" ? model.supportsImages === true : (model.fileMimeTypes?.length ?? 0) > 0;
}

export function matchesModelFilters(model: ProviderModel, filters: ModelFilters): boolean {
  if (filters.query) {
    const haystack = [
      model.label,
      model.shortLabel,
      model.id,
      model.model,
      model.providerLabel,
      ...(model.tags ?? []).map((tag) => tag.label),
    ].join(" ").toLocaleLowerCase();
    if (!haystack.includes(filters.query)) return false;
  }
  for (const tag of filters.tags) {
    if (!model.tags?.some((candidate) => candidate.id === tag)) return false;
  }
  for (const capability of filters.capabilities) {
    if (!modelHasCapability(model, capability)) return false;
  }
  return filters.hosts.size === 0 || filters.hosts.has(model.providerId);
}

export function filterModels(models: readonly ProviderModel[], filters: ModelFilters): ProviderModel[] {
  return models.filter((model) => matchesModelFilters(model, filters)).sort(compareModelPreference);
}

/** Number of models that would remain after applying `patch` to `filters`. */
export function countModels(
  models: readonly ProviderModel[],
  filters: ModelFilters,
  patch: Partial<ModelFilters> = {},
): number {
  const next = { ...filters, ...patch };
  return models.reduce((count, model) => count + Number(matchesModelFilters(model, next)), 0);
}

/** Every catalog tag once, with Uncensored first, then catalog position and label. */
export function catalogTags(models: readonly ProviderModel[]): ModelTag[] {
  const tags = new Map<string, ModelTag>();
  for (const model of models) {
    for (const tag of model.tags ?? []) {
      const existing = tags.get(tag.id);
      if (!existing || tag.position < existing.position) tags.set(tag.id, tag);
    }
  }
  return [...tags.values()].sort((left, right) => Number(right.id === "uncensored") - Number(left.id === "uncensored")
    || left.position - right.position
    || left.label.localeCompare(right.label)
    || left.id.localeCompare(right.id));
}

/** Hosts in the same preference order the model list uses. */
export function catalogHosts(models: readonly ProviderModel[]): HostOption[] {
  const hosts = new Map<string, HostOption>();
  for (const model of [...models].sort(compareModelPreference)) {
    if (!hosts.has(model.providerId)) hosts.set(model.providerId, { id: model.providerId, label: model.providerLabel });
  }
  return [...hosts.values()];
}

/** Capabilities worth offering as filters: some models have them and some do not. */
export function catalogCapabilities(models: readonly ProviderModel[]): ModelCapability[] {
  return (["images", "files"] as const).filter((capability) => {
    const supported = models.filter((model) => modelHasCapability(model, capability)).length;
    return supported > 0 && supported < models.length;
  });
}

export function toggled<T>(values: ReadonlySet<T>, value: T): Set<T> {
  const next = new Set(values);
  if (!next.delete(value)) next.add(value);
  return next;
}

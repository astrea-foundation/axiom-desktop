export type ReasoningEffort = "provider_default" | "enabled" | "disabled" | "minimal" | "low" | "medium" | "high" | "xhigh";

export type ModelTagTone = "gray" | "green" | "blue" | "violet" | "amber" | "pink" | "orange" | "teal" | "red";

/** Backend-curated model label, such as "Cheap", "Uncensored" or "New". The native runtime uses the "uncensored" tag to pick the uncensored image adapter; tags never affect routing or authorization. */
export interface ModelTag {
  id: string;
  label: string;
  tone: ModelTagTone;
  position: number;
}

export interface ProviderModel {
  id: string;
  label: string;
  shortLabel: string;
  providerId: string;
  providerLabel: string;
  model: string;
  supportedReasoningEfforts: ReasoningEffort[];
  inputPriceMicrousdPerMillionTokens: number | null;
  outputPriceMicrousdPerMillionTokens: number | null;
  contextWindowTokens?: number;
  supportsImages?: boolean;
  fileMimeTypes?: string[];
  autoCompactThresholdTokens?: number;
  /** Sorted by position, then ID. Absent for catalogs that predate tags. */
  tags?: ModelTag[];
}


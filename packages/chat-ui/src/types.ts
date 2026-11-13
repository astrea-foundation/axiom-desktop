export type ReasoningEffort = "provider_default" | "enabled" | "disabled" | "minimal" | "low" | "medium" | "high" | "xhigh";

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
}


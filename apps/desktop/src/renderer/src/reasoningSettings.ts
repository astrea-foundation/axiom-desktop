import type { ReasoningEffort } from "./types";

const KNOWN: readonly ReasoningEffort[] = ["provider_default", "enabled", "disabled", "minimal", "low", "medium", "high", "xhigh"];

export function isReasoningEffort(value: string | undefined): value is ReasoningEffort {
  return KNOWN.includes(value as ReasoningEffort);
}

/** Ignore the removed option even when connected to an older sidecar. */
export function explicitReasoningEfforts(supported: readonly ReasoningEffort[]): ReasoningEffort[] {
  return supported.filter((effort) => effort !== "provider_default");
}

/** Keep valid choices; resolve stale preferences against this exact offering. */
export function reconcileReasoningEffort(
  preferred: string | undefined,
  supported: readonly ReasoningEffort[],
): ReasoningEffort {
  const options = explicitReasoningEfforts(supported);
  if (isReasoningEffort(preferred) && options.includes(preferred)) return preferred;
  if (options.includes("enabled")) return "enabled";
  if (options.includes("medium")) return "medium";
  // No controls means no selector and no wire parameter, not a fabricated Off.
  return options[0] ?? "provider_default";
}

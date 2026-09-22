import type { ProviderModel } from "../types";

function formatUsdMicrousd(microusd: number): string {
  const dollars = Math.floor(microusd / 1_000_000);
  const fraction = String(microusd % 1_000_000).padStart(6, "0").replace(/0+$/, "");
  return fraction ? `$${dollars}.${fraction}` : `$${dollars}`;
}

export function formatModelPricing(model: Pick<ProviderModel,
  "inputPriceMicrousdPerMillionTokens" | "outputPriceMicrousdPerMillionTokens"
>): string | null {
  const input = model.inputPriceMicrousdPerMillionTokens;
  const output = model.outputPriceMicrousdPerMillionTokens;
  if (input === null || output === null) return null;
  return `${formatUsdMicrousd(input)} in, ${formatUsdMicrousd(output)} out`;
}

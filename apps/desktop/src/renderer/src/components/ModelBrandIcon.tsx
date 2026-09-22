import { Box } from "lucide-react";
import tinfoilIcon from "../../../../resources/licenses/tinfoil-logo/tinfoil.svg?url";
import alibabaIcon from "@lobehub/icons-static-svg/icons/alibaba-color.svg?url";
import anthropicIcon from "@lobehub/icons-static-svg/icons/anthropic.svg?url";
import cohereIcon from "@lobehub/icons-static-svg/icons/cohere-color.svg?url";
import deepSeekIcon from "@lobehub/icons-static-svg/icons/deepseek-color.svg?url";
import gemmaIcon from "@lobehub/icons-static-svg/icons/gemma-color.svg?url";
import googleIcon from "@lobehub/icons-static-svg/icons/google-color.svg?url";
import metaIcon from "@lobehub/icons-static-svg/icons/meta-color.svg?url";
import minimaxIcon from "@lobehub/icons-static-svg/icons/minimax-color.svg?url";
import mistralIcon from "@lobehub/icons-static-svg/icons/mistral-color.svg?url";
import moonshotIcon from "@lobehub/icons-static-svg/icons/moonshot.svg?url";
import nvidiaIcon from "@lobehub/icons-static-svg/icons/nvidia-color.svg?url";
import openAiIcon from "@lobehub/icons-static-svg/icons/openai.svg?url";
import qwenIcon from "@lobehub/icons-static-svg/icons/qwen-color.svg?url";
import xAiIcon from "@lobehub/icons-static-svg/icons/xai.svg?url";
import zAiIcon from "@lobehub/icons-static-svg/icons/zai.svg?url";
import type { ProviderModel } from "../types";

interface BrandAsset {
  name: string;
  src: string;
  monochrome?: boolean;
}

const MODEL_BRANDS: Array<{ aliases: string[]; asset: BrandAsset }> = [
  { aliases: ["deepseek"], asset: { name: "DeepSeek", src: deepSeekIcon } },
  { aliases: ["gemma"], asset: { name: "Gemma", src: gemmaIcon } },
  { aliases: ["z ai", "zai", "zhipu", "chatglm", "glm"], asset: { name: "Z.ai", src: zAiIcon, monochrome: true } },
  { aliases: ["openai", "gpt"], asset: { name: "OpenAI", src: openAiIcon, monochrome: true } },
  { aliases: ["qwen"], asset: { name: "Qwen", src: qwenIcon } },
  { aliases: ["alibaba", "tongyi"], asset: { name: "Alibaba", src: alibabaIcon } },
  { aliases: ["anthropic", "claude"], asset: { name: "Anthropic", src: anthropicIcon, monochrome: true } },
  { aliases: ["google", "gemini"], asset: { name: "Google", src: googleIcon } },
  { aliases: ["meta", "llama"], asset: { name: "Meta", src: metaIcon } },
  { aliases: ["mistral", "mixtral"], asset: { name: "Mistral AI", src: mistralIcon } },
  { aliases: ["moonshot", "kimi"], asset: { name: "Moonshot AI", src: moonshotIcon, monochrome: true } },
  { aliases: ["minimax"], asset: { name: "MiniMax", src: minimaxIcon } },
  { aliases: ["cohere", "command r", "command a"], asset: { name: "Cohere", src: cohereIcon } },
  { aliases: ["xai", "x ai", "grok"], asset: { name: "xAI", src: xAiIcon, monochrome: true } },
  { aliases: ["nvidia", "nemotron"], asset: { name: "NVIDIA", src: nvidiaIcon } },
];

export function normalizeCompanyName(value: string): string {
  return value
    .normalize("NFKD")
    .toLocaleLowerCase()
    .replace(/&/g, " and ")
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

function hasAlias(value: string, alias: string): boolean {
  return ` ${value} `.includes(` ${alias} `);
}

function resolveBrand(value: string): BrandAsset | null {
  const normalized = normalizeCompanyName(value);
  return MODEL_BRANDS.find(({ aliases }) => aliases.some((alias) => hasAlias(normalized, alias)))
    ?.asset ?? null;
}

function AssetIcon({ asset, className }: { asset: BrandAsset; className: string }) {
  return (
    <img
      src={asset.src}
      alt=""
      aria-hidden="true"
      title={asset.name}
      className={`${className} object-contain ${asset.monochrome ? "brand-mono" : ""}`}
    />
  );
}

function NearAiMark({ className }: { className: string }) {
  return (
    <svg viewBox="0 0 103 103" aria-hidden="true" className={className}>
      <rect width="103" height="103" rx="14.292" className="fill-[var(--surface-tile)]" />
      <path
        d="M75.217 21.364c-2.23 0-4.307 1.157-5.474 3.061L57.142 43.128a1.344 1.344 0 0 0 2.003 1.76l12.388-10.756a.501.501 0 0 1 .836.381V68.19a.504.504 0 0 1-.89.322L33.993 23.638a6.42 6.42 0 0 0-4.901-2.275h-1.31a6.419 6.419 0 0 0-6.419 6.419v47.431a6.419 6.419 0 0 0 11.893 3.357l12.601-18.703a1.344 1.344 0 0 0-2.003-1.76L31.466 68.863a.501.501 0 0 1-.836-.381V34.795a.504.504 0 0 1 .891-.322l37.48 44.884a6.42 6.42 0 0 0 4.901 2.275h1.31a6.424 6.424 0 0 0 6.424-6.419V27.787a6.419 6.419 0 0 0-6.419-6.419Z"
        fill="#0091fd"
      />
    </svg>
  );
}

function GenericBrandIcon({ className }: { className: string }) {
  return <Box aria-hidden="true" className={className} strokeWidth={1.8} />;
}

export function ModelBrandIcon({ model, className = "h-4 w-4" }: {
  model: ProviderModel;
  className?: string;
}) {
  const asset = resolveBrand(`${model.label} ${model.shortLabel} ${model.id} ${model.model}`);
  return asset
    ? <AssetIcon asset={asset} className={className} />
    : <GenericBrandIcon className={className} />;
}

export function ProviderBrandIcon({ providerId, providerLabel, className = "h-3.5 w-3.5" }: {
  providerId: string;
  providerLabel: string;
  className?: string;
}) {
  const normalized = normalizeCompanyName(`${providerId} ${providerLabel}`);
  if (hasAlias(normalized, "tinfoil")) {
    return <AssetIcon asset={{ name: "Tinfoil", src: tinfoilIcon, monochrome: true }} className={className} />;
  }
  if (hasAlias(normalized, "near") || hasAlias(normalized, "near ai")) {
    return <NearAiMark className={className} />;
  }
  const asset = resolveBrand(normalized);
  return asset
    ? <AssetIcon asset={asset} className={className} />
    : <GenericBrandIcon className={className} />;
}

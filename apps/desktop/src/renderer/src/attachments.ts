import { MAX_IMAGE_BYTES, MAX_FILE_BYTES, validatePrompt, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";
import type { ProviderModel } from "./types";

const imageTypes = new Map([["png", "image/png"], ["jpg", "image/jpeg"], ["jpeg", "image/jpeg"], ["webp", "image/webp"], ["gif", "image/gif"]]);
const fileTypes = new Map([
  ["txt", "text/plain"], ["md", "text/markdown"], ["markdown", "text/markdown"], ["csv", "text/csv"],
  ["html", "text/html"], ["htm", "text/html"], ["json", "application/json"], ["xml", "application/xml"],
  ["pdf", "application/pdf"],
  ["docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
  ["pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"],
  ["xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
]);
const codeExtensions = new Set("tsv jsonl yaml yml toml css scss js jsx mjs cjs ts tsx py rs go java c h cpp hpp cs rb php sh bash zsh fish sql log ini conf cfg env tex rst diff patch svelte vue swift kt lua r dart dockerfile makefile gitignore".split(" "));

export function acceptsAttachment(model: ProviderModel | undefined, attachment: PromptAttachment): boolean {
  if (attachment.kind === "image") return Boolean(model?.supportsImages);
  if (attachment.kind === "file") return model?.fileMimeTypes?.includes(attachment.file.mimeType) ?? false;
  return false;
}

export function attachmentAccept(model: ProviderModel | undefined): string {
  const types = new Set(model?.fileMimeTypes ?? []);
  if (model?.supportsImages) for (const mime of imageTypes.values()) types.add(mime);
  return [...types].join(",") || ".unsupported";
}

/** Read original bytes only. No extraction, OCR, conversion, or prompt injection. */
export async function readAttachment(file: File): Promise<PromptAttachment> {
  const extension = file.name.toLowerCase().split(".").at(-1) ?? "";
  const imageMime = imageTypes.get(extension) ?? (Array.from(imageTypes.values()).includes(file.type) ? file.type : undefined);
  const mimeType = imageMime ?? fileTypes.get(extension) ?? (codeExtensions.has(extension) ? "text/plain" : undefined);
  if (!mimeType) throw new Error(`${file.name}: unsupported upload format.`);
  const limit = imageMime ? MAX_IMAGE_BYTES : MAX_FILE_BYTES;
  if (!file.size || file.size > limit) throw new Error(`${file.name}: use a file up to ${limit / 1024 / 1024} MiB.`);
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  const data = btoa(binary);
  const attachment: PromptAttachment = imageMime
    ? { kind: "image", name: file.name, image: { mimeType, data } }
    : { kind: "file", name: file.name, file: { name: file.name, mimeType, data } };
  validatePrompt("", [attachment]);
  return attachment;
}

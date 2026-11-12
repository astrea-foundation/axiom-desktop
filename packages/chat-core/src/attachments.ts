import { MAX_IMAGE_BYTES, MAX_FILE_BYTES, validatePrompt, type PromptAttachment } from "./attachmentValidation";
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
  if (attachment.kind === "image") return model?.supportsImages === true;
  if (attachment.kind === "file") return model?.fileMimeTypes?.includes(attachment.file.mimeType) ?? false;
  return false;
}

export function attachmentAccept(model: ProviderModel | undefined): string {
  const types = new Set(model?.fileMimeTypes ?? []);
  if (model?.supportsImages === true) for (const mime of imageTypes.values()) types.add(mime);
  return [...types].join(",") || ".unsupported";
}

/** Keep generation-sized originals locally; providers prepare their own vision copies. */
export async function readAttachment(file: File, options: { maxImagePixels?: number } = {}): Promise<PromptAttachment> {
  const extension = file.name.toLowerCase().split(".").at(-1) ?? "";
  const imageMime = imageTypes.get(extension) ?? (Array.from(imageTypes.values()).includes(file.type) ? file.type : undefined);
  let mimeType = imageMime ?? fileTypes.get(extension) ?? (codeExtensions.has(extension) ? "text/plain" : undefined);
  if (!mimeType) throw new Error(`${file.name}: unsupported upload format.`);
  const limit = imageMime ? 32 * 1024 * 1024 : MAX_FILE_BYTES;
  if (!file.size || file.size > limit) throw new Error(`${file.name}: use a file up to ${limit / 1024 / 1024} MiB.`);
  let bytes: Uint8Array;
  if (imageMime) {
    const bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
    try {
      if (bitmap.width > 16384 || bitmap.height > 16384 || bitmap.width * bitmap.height > 64 * 1024 * 1024) throw new Error(`${file.name}: image dimensions are too large.`);
      const budget = options.maxImagePixels ?? 2048 * 2048;
      if (!Number.isInteger(budget) || budget < 1 || budget > 2048 * 2048) throw new Error("Invalid local image budget.");
      const scale = Math.min(1, Math.sqrt(budget / (bitmap.width * bitmap.height)), 8192 / Math.max(bitmap.width, bitmap.height));
      const canvas = document.createElement("canvas");
      const grid = options.maxImagePixels === undefined ? 32 : 1;
      canvas.width = Math.max(grid, Math.floor(bitmap.width * scale / grid) * grid);
      canvas.height = Math.max(grid, Math.floor(bitmap.height * scale / grid) * grid);
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) throw new Error("Local image resizing is unavailable.");
      context.imageSmoothingEnabled = true;
      context.imageSmoothingQuality = "high";
      context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
      // PNG avoids an extra lossy JPEG encode before reference conditioning.
      mimeType = "image/png";
      const normalized = await new Promise<Blob>((resolve, reject) => canvas.toBlob(
        blob => blob ? resolve(blob) : reject(new Error("Local image resizing failed.")), mimeType));
      if (normalized.size > MAX_IMAGE_BYTES) throw new Error(`${file.name}: normalized image exceeds 20 MiB.`);
      bytes = new Uint8Array(await normalized.arrayBuffer());
    } finally { bitmap.close(); }
  } else {
    bytes = new Uint8Array(await file.arrayBuffer());
  }
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  const data = btoa(binary);
  const attachment: PromptAttachment = imageMime
    ? { kind: "image", name: file.name, image: { mimeType, data } }
    : { kind: "file", name: file.name, file: { name: file.name, mimeType, data } };
  validatePrompt("", [attachment]);
  return attachment;
}

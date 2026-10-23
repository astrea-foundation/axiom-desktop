import type { PromptAttachment } from "./generated/protocol.js";

export const MAX_PROMPT_BYTES = 4 * 1024 * 1024;
export const MAX_ATTACHMENT_BYTES = 16 * 1024 * 1024;
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024;
export const MAX_ATTACHMENTS = 8;
const bytes = (text: string) => new TextEncoder().encode(text).length;

export function promptBytes(text: string, attachments: readonly PromptAttachment[]): number {
  return bytes(text) + attachments.reduce((sum, file) => sum + (file.kind === "text" ? bytes(file.text) : file.image.data.length), 0);
}

/** Shared renderer/main/client validation; native validation is authoritative. */
export function validatePrompt(text: unknown, attachments: unknown, allowEmpty = false): asserts attachments is PromptAttachment[] {
  if (typeof text !== "string" || bytes(text) > MAX_PROMPT_BYTES) throw new Error("Prompt text must fit within 4 MiB of UTF-8.");
  if (!Array.isArray(attachments) || attachments.length > MAX_ATTACHMENTS) throw new Error("Attach at most 8 files per message.");
  if (!allowEmpty && !text.trim() && !attachments.length) throw new Error("Write a message or attach a file.");
  let combinedText = text;
  for (const file of attachments) {
    if (!file || typeof file !== "object" || typeof file.name !== "string" || !file.name || bytes(file.name) > 512 || /[\u0000-\u001f\u007f-\u009f]/u.test(file.name)) throw new Error("Invalid attachment name.");
    if (file.kind === "text") {
      throw new Error(`${file.name}: text and document attachments are not supported for upload. Remove this attachment before sending.`);
    } else if (file.kind === "image") {
      const image = file.image;
      if (!image || !["image/png", "image/jpeg", "image/webp", "image/gif"].includes(image.mimeType) || typeof image.data !== "string"
        || !image.data.length || image.data.length > Math.ceil(MAX_IMAGE_BYTES / 3) * 4
        || (image.data.length % 4 !== 0 || /[^A-Za-z0-9+/]/u.test(image.data.replace(/={1,2}$/u, "")))) throw new Error(`${file.name}: use a PNG, JPEG, WebP, or GIF image up to 5 MiB.`);
      const rawSize = image.data.length / 4 * 3 - (image.data.endsWith("==") ? 2 : image.data.endsWith("=") ? 1 : 0);
      if (rawSize > MAX_IMAGE_BYTES) throw new Error(`${file.name}: image exceeds 5 MiB.`);
      combinedText += `\n[Attached image ${JSON.stringify(file.name)}]`;
    } else throw new Error("Unsupported attachment type.");
  }
  if (bytes(combinedText) > MAX_PROMPT_BYTES) throw new Error("The message and image names together must fit within 4 MiB.");
  if (promptBytes(text, attachments) > MAX_ATTACHMENT_BYTES) throw new Error("The message and encoded images together must fit within 16 MiB.");
}

export type { PromptAttachment } from "./generated/protocol.js";

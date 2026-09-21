import { MAX_IMAGE_BYTES, MAX_PROMPT_BYTES, validatePrompt, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";

const imageTypes = new Map([["png", "image/png"], ["jpg", "image/jpeg"], ["jpeg", "image/jpeg"], ["webp", "image/webp"], ["gif", "image/gif"]]);
const textExtensions = new Set("txt md markdown csv tsv json jsonl yaml yml toml xml html htm css scss js jsx mjs cjs ts tsx py rs go java c h cpp hpp cs rb php sh bash zsh fish sql log ini conf cfg env tex rst diff patch svelte vue swift kt lua r dart dockerfile makefile gitignore".split(" "));

export async function readAttachment(file: File): Promise<PromptAttachment> {
  const extension = file.name.toLowerCase().split(".").at(-1) ?? "";
  const mimeType = imageTypes.get(extension) ?? (Array.from(imageTypes.values()).includes(file.type) ? file.type : undefined);
  let attachment: PromptAttachment;
  if (mimeType) {
    if (!file.size || file.size > MAX_IMAGE_BYTES) throw new Error(`${file.name}: images must be no larger than 5 MiB.`);
    const bytes = new Uint8Array(await file.arrayBuffer());
    const bitmap = await createImageBitmap(file).catch(() => { throw new Error(`${file.name}: this image could not be decoded.`); });
    const pixels = bitmap.width * bitmap.height;
    bitmap.close();
    if (pixels > 40_000_000) throw new Error(`${file.name}: use an image with at most 40 million pixels.`);
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
    attachment = { kind: "image", name: file.name, image: { mimeType, data: btoa(binary) } };
  } else if (extension === "pdf" || file.type === "application/pdf") {
    if (!file.size || file.size > 20 * 1024 * 1024) throw new Error(`${file.name}: PDFs must be no larger than 20 MiB.`);
    const { extractPdf } = await import("./pdfAttachments");
    attachment = { kind: "text", name: file.name, text: await extractPdf(new Uint8Array(await file.arrayBuffer())) };
  } else {
    if (!textExtensions.has(extension) && !file.type.startsWith("text/") && !["application/json", "application/xml"].includes(file.type)) throw new Error(`${file.name}: attach an image, PDF, or text/code file. Export Office documents to PDF or text first.`);
    if (!file.size || file.size > MAX_PROMPT_BYTES) throw new Error(`${file.name}: text files must be no larger than 4 MiB.`);
    let text: string;
    try { text = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer()); }
    catch { throw new Error(`${file.name}: save this text file as UTF-8 before attaching it.`); }
    if (!text.trim() || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/u.test(text)) throw new Error(`${file.name}: this file is empty or contains binary data.`);
    attachment = { kind: "text", name: file.name, text };
  }
  validatePrompt("", [attachment]);
  return attachment;
}

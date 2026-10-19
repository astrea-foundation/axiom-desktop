import { MAX_IMAGE_BYTES, validatePrompt, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";

const imageTypes = new Map([["png", "image/png"], ["jpg", "image/jpeg"], ["jpeg", "image/jpeg"], ["webp", "image/webp"], ["gif", "image/gif"]]);
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
  } else {
    throw new Error(`${file.name}: this file type isn’t supported for upload. Attach a PNG, JPEG, WebP, or GIF image with a supported model.`);
  }
  validatePrompt("", [attachment]);
  return attachment;
}

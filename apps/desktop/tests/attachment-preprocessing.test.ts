import assert from "node:assert/strict";
import test from "node:test";
import { readAttachment } from "../../../packages/chat-core/src/attachments";

test("generation attachments retain the 2K budget on the model grid without JPEG loss", async () => {
  const originalBitmap = Object.getOwnPropertyDescriptor(globalThis, "createImageBitmap");
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const context = { imageSmoothingEnabled: false, imageSmoothingQuality: "low", drawImage() {} };
  let format: string | undefined;
  const canvas = { width: 0, height: 0, getContext: () => context,
    toBlob: (done: (blob: Blob) => void, mime: string) => { format = mime; done(new Blob(["png"], { type: mime })); } };
  Object.defineProperty(globalThis, "createImageBitmap", { configurable: true, value: async () => ({ width: 3000, height: 4000, close() {} }) });
  Object.defineProperty(globalThis, "document", { configurable: true, value: { createElement: () => canvas } });
  try {
    const image = await readAttachment(new File(["photo"], "photo.jpg", { type: "image/jpeg" }));
    assert.equal(image.kind, "image");
    assert.equal(format, "image/png");
    assert.equal(context.imageSmoothingQuality, "high");
    assert.ok(context.imageSmoothingEnabled);
    assert.equal(canvas.width % 32, 0); assert.equal(canvas.height % 32, 0);
    assert.ok(canvas.width * canvas.height > 1024 * 1024);
    assert.ok(canvas.width * canvas.height <= 2048 * 2048);
    await readAttachment(new File(["photo"], "photo.jpg", { type: "image/jpeg" }), { maxImagePixels: 1024 * 1024 });
    assert.ok(canvas.width * canvas.height <= 1024 * 1024);
  } finally {
    if (originalBitmap) Object.defineProperty(globalThis, "createImageBitmap", originalBitmap);
    else Reflect.deleteProperty(globalThis, "createImageBitmap");
    if (originalDocument) Object.defineProperty(globalThis, "document", originalDocument);
    else Reflect.deleteProperty(globalThis, "document");
  }
});

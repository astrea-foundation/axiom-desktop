import assert from "node:assert/strict";
import test from "node:test";
import { validatePrompt, MAX_PROMPT_BYTES, MAX_IMAGE_BYTES } from "../src/attachments.js";

test("UTF-8 and aggregate attachment limits are enforced without the old 64 KiB cap", () => {
  assert.doesNotThrow(() => validatePrompt("x".repeat(256 * 1024), []));
  assert.throws(() => validatePrompt("界".repeat(MAX_PROMPT_BYTES / 3 + 1), []));
  assert.throws(() => validatePrompt("x".repeat(MAX_PROMPT_BYTES), [{ kind: "text", name: "f", text: "x" }]));
  assert.doesNotThrow(() => validatePrompt("", [{ kind: "text", name: "notes", text: "data" }]));
  assert.throws(() => validatePrompt("", []));
});

test("multi-megabyte image validation is bounded and accepts no remote image references", () => {
  const data = Buffer.alloc(MAX_IMAGE_BYTES).toString("base64");
  assert.doesNotThrow(() => validatePrompt("", [{ kind: "image", name: "image.png", image: { mimeType: "image/png", data } }]));
  assert.throws(() => validatePrompt("", [{ kind: "image", name: "image.png", image: { mimeType: "image/png", data: "https://example.com/private" } }]));
  assert.throws(() => validatePrompt("", [{ kind: "image", name: "image.svg", image: { mimeType: "image/svg+xml", data } }]));
});

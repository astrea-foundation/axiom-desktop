import assert from "node:assert/strict";
import { test } from "node:test";
import { IMAGE_ATTACHMENT_ACCEPT, attachmentAffordance } from "../src/renderer/src/attachmentAffordance";
import { readAttachment } from "../src/renderer/src/attachments";

const model = (supportsImages: boolean) => ({ shortLabel: "GLM 5.3 Flash", providerLabel: "NearAI", supportsImages });

test("models with native encrypted image input allow only supported image formats", () => {
  const affordance = attachmentAffordance(model(true));
  assert.equal(affordance.blocked, false);
  assert.equal(affordance.accept, IMAGE_ATTACHMENT_ACCEPT);
  assert.match(affordance.hint, /Attach images/);
  assert.doesNotMatch(affordance.accept, /pdf|text|svg|image\/\*/);
});

test("models without upload support disable attachments and explain why", () => {
  const affordance = attachmentAffordance(model(false));
  assert.equal(affordance.blocked, true);
  assert.equal(affordance.hint, "NearAI’s GLM 5.3 Flash doesn’t support attachments.");
  assert.equal(attachmentAffordance({ shortLabel: "Unknown", providerLabel: "Unknown provider" }).blocked, true);
  assert.equal(attachmentAffordance(undefined).blocked, true);
});

test("connection and account restrictions block attaching even with a supported model", () => {
  for (const current of [model(true), model(false), undefined]) {
    const affordance = attachmentAffordance(current, "Sign in to attach files.");
    assert.equal(affordance.blocked, true);
    assert.equal(affordance.hint, "Sign in to attach files.");
  }
});

test("documents and source files are rejected before reading or extracting their contents", async () => {
  for (const [name, type] of [["notes.pdf", "application/pdf"], ["notes.txt", "text/plain"], ["app.ts", "text/plain"], ["notes.docx", "application/octet-stream"]]) {
    const file = new File(["private file contents"], name!, { type });
    file.arrayBuffer = async () => { throw new Error("File must not be read"); };
    await assert.rejects(readAttachment(file), /isn’t supported for upload/);
  }
});

import assert from "node:assert/strict";
import { test } from "node:test";
import type { PromptAttachment } from "@axiom/axiom-acp-client";
import { AttachmentPreviews } from "../src/renderer/src/attachmentPreviews";

const files: PromptAttachment[] = [{ kind: "image", name: "pixel.png", image: { mimeType: "image/png", data: "YWJj" } }];

test("preview handoff is bounded and keeps separate account/runtime instances isolated", () => {
  const first = new AttachmentPreviews(8);
  const second = new AttachmentPreviews(8);
  first.remember("one", files);
  first.remember("one", files);
  first.remember("two", files);
  assert.equal(first.get("one"), files, "remembering twice does not consume more space");
  first.remember("three", files);
  assert.equal(first.get("one"), undefined);
  assert.equal(first.get("two"), files);
  assert.equal(first.get("three"), files);
  assert.equal(second.get("three"), undefined);
  first.remember("too-large", [...files, ...files, ...files]);
  assert.equal(first.get("too-large"), undefined);
  assert.equal(first.get("three"), files);
});

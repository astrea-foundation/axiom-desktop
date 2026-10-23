import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PreparingThreadView } from "../src/renderer/src/components/PreparingThreadView";

test("a preparing thread shows the real user bubble and an accessible sending state", () => {
  const html = renderToStaticMarkup(createElement(PreparingThreadView, { id: "local-only", text: "Prepare my workspace" }));
  assert.match(html, /aria-label="Chat messages"/);
  assert.match(html, /aria-busy="true"/);
  assert.match(html, /data-timeline-kind="user"/);
  assert.match(html, /Prepare my workspace/);
  assert.match(html, /role="status"/);
  assert.match(html, /Sending…/);
  assert.doesNotMatch(html, /Generating|Loading thread|Edit message|TEE verified/);
});

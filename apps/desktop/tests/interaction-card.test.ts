import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { InteractionCard } from "../src/renderer/src/components/InteractionCard";
import type { PendingInteraction } from "@axiom/axiom-acp-client";

test("approvals display concrete action details as text, without duplicate Deny buttons", () => {
  const interaction = { id: "permission", kind: "permission", payload: {
    toolCall: { title: "Approve command:\nrun echo <script> in /My Project" },
    options: [{ optionId: "allow_once", name: "Allow once", kind: "allow_once" }, { optionId: "reject_once", name: "Deny", kind: "reject_once" }],
  } } as PendingInteraction;
  const html = renderToStaticMarkup(createElement(InteractionCard, { interaction }));
  assert.match(html, /run echo &lt;script&gt; in \/My Project/);
  assert.equal(html.match(/>Deny<\/button>/g)?.length, 1);
  assert.doesNotMatch(html, /<script>/);
});

import assert from "node:assert/strict";
import test from "node:test";
import type { ClientTimelineItem } from "@axiom/axiom-acp-client";
import { presentTimeline } from "../src/renderer/src/components/timelinePresentation";

function item(
  kind: ClientTimelineItem["kind"],
  id: string,
  turnId: string,
  text: string,
  status = "completed",
): ClientTimelineItem {
  return { id, turnId, kind, text, status };
}

test("authoritative assistant-before-reasoning records render as one response", () => {
  const rows = presentTimeline([
    item("assistant", "assistant-row", "turn-1", "final answer"),
    item("reasoning", "reasoning-row", "turn-1", "private thought"),
  ]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.kind, "message");
  if (rows[0]?.kind !== "message") return;
  assert.equal(rows[0].message.content, "final answer");
  assert.equal(rows[0].message.reasoningContent, "private thought");
  assert.equal(rows[0].message.status, "complete");
});

test("live reasoning-before-assistant records render as one response", () => {
  const rows = presentTimeline([
    item("reasoning", "reasoning:turn-1", "turn-1", "private thought", "streaming"),
    item("assistant", "assistant:turn-1", "turn-1", "partial answer", "streaming"),
  ]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.kind, "message");
  if (rows[0]?.kind !== "message") return;
  assert.equal(rows[0].message.content, "partial answer");
  assert.equal(rows[0].message.reasoningContent, "private thought");
  assert.equal(rows[0].message.status, "streaming");
});

test("turn identity prevents unrelated response rows from being merged", () => {
  const rows = presentTimeline([
    item("assistant", "assistant-1", "turn-1", "one"),
    item("reasoning", "reasoning-2", "turn-2", "two"),
  ]);
  assert.equal(rows.length, 2);
});

test("empty terminal placeholders disappear but failures remain explicit", () => {
  const rows = presentTimeline([
    item("assistant", "empty", "turn-1", ""),
    item("assistant", "failed", "turn-2", "unverified partial", "failed"),
  ]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.kind, "message");
  if (rows[0]?.kind !== "message") return;
  assert.equal(rows[0].message.status, "failed");
  assert.equal(rows[0].message.terminalVerified, false);
});

test("a per-item terminal marker is preserved without consulting session preflight", () => {
  const assistant = {
    ...item("assistant", "verified", "turn-1", "verified answer"),
    terminalVerified: true,
  };
  const rows = presentTimeline([assistant]);
  assert.equal(rows[0]?.kind, "message");
  if (rows[0]?.kind !== "message") return;
  assert.equal(rows[0].message.terminalVerified, true);
});

test("empty process-state notices do not create blank transcript cards", () => {
  const rows = presentTimeline([
    item("activity", "security-state", "", ""),
    item("activity", "provider-state", "", "Provider connected"),
    item("error", "failed-state", "", "", "failed"),
  ]);
  assert.deepEqual(
    rows.map((row) => row.key),
    ["provider-state", "failed-state"],
  );
});

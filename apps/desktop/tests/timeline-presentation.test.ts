import assert from "node:assert/strict";
import test from "node:test";
import type { ClientTimelineItem } from "@axiom/axiom-acp-client";
import { messageRevisionSources, presentTimeline } from "../src/renderer/src/components/timelinePresentation";

function item(
  kind: ClientTimelineItem["kind"],
  id: string,
  turnId: string,
  text: string,
  status = "completed",
): ClientTimelineItem {
  return { id, turnId, kind, text, status };
}

test("regeneration after steering starts from the original turn prompt", () => {
  const original = item("user", "prompt", "turn-1", "Original prompt");
  const next = item("user", "next", "turn-2", "Next prompt");
  const sources = messageRevisionSources([
    original,
    item("assistant", "partial", "turn-1", "Before steering"),
    { ...item("user", "steer", "turn-1", "Steering"), raw: { metadata: { steering: true } } },
    item("assistant", "final", "turn-1", "After steering"),
    next,
    item("assistant", "next-reply", "turn-2", "Next reply"),
    item("assistant", "orphan", "unknown-turn", "No original prompt"),
  ]);
  assert.equal(sources.get("prompt"), original);
  assert.equal(sources.get("partial"), original);
  assert.equal(sources.get("final"), original);
  assert.equal(sources.get("next-reply"), next);
  assert.equal(sources.has("steer"), false);
  assert.equal(sources.has("orphan"), false);
});

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

test("routine security notices do not split reasoning from the assistant", () => {
  const rows = presentTimeline([
    item("assistant", "assistant-row", "turn-1", "", "in_progress"),
    {
      ...item("activity", "provider-status", "", "Verifying model attestation"),
      raw: { kind: "notice", metadata: { code: "provider_status" } },
    },
    item("reasoning", "provider:17", "", "[provider] Verifying model attestation", "streaming"),
    {
      ...item("activity", "security-status", "", "TEE verified"),
      raw: { kind: "notice", metadata: { code: "security_status" } },
    },
    item("reasoning", "reasoning-row", "turn-1", "thinking", "in_progress"),
  ]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.kind, "message");
  if (rows[0]?.kind !== "message") return;
  assert.equal(rows[0].message.reasoningContent, "thinking");
  assert.equal(rows[0].message.status, "streaming");
});

test("provider failures and ordinary content mentioning verification remain visible", () => {
  const timeline: ClientTimelineItem[] = [
    {
      ...item("error", "provider-error", "", "Connection failed", "failed"),
      raw: { kind: "notice", metadata: { code: "provider_status", connected: false } },
    },
    item("reasoning", "provider:18", "", "[provider status] Connection failed", "streaming"),
    item("assistant", "reply", "turn-1", "Verifying model attestation"),
    item("activity", "warning", "", "Verifying model attestation"),
  ];
  assert.deepEqual(presentTimeline(timeline).map((row) => row.key), timeline.map((row) => row.id));
});


test("permission cards own pending decisions; redundant approval notices stay out of the transcript", () => {
  const timeline: ClientTimelineItem[] = [
    { ...item("activity", "pending-permission", "", "Allow read file?", "pending"), raw: { kind: "notice", metadata: { code: "permission" } } },
    { ...item("activity", "allowed-permission", "", "Allow read file?", "completed"), raw: { kind: "notice", metadata: { code: "permission", allowed: true } } },
    { ...item("error", "denied-permission", "", "You denied this action", "failed"), raw: { kind: "notice", metadata: { code: "permission", allowed: false } } },
  ];
  assert.deepEqual(presentTimeline(timeline).map((row) => row.key), ["denied-permission"]);
});

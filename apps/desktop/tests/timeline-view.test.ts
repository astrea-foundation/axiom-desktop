import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { ClientSessionState, ClientTimelineItem } from "@axiom/axiom-acp-client";
import { TimelineView } from "../src/renderer/src/components/TimelineView";
import { TeeVerificationBadge } from "../src/renderer/src/components/TeeVerificationBadge";

function session(timeline: ClientTimelineItem[]): ClientSessionState {
  return {
    sessionId: "thread-1", title: null, cwd: "", settings: null,
    security: { state: "verifying" }, modes: [], currentModeId: null,
    configOptions: [], timeline, interactions: [], running: true,
    needsResync: false, threadRevision: 1, lastTimelineSequence: 0,
  };
}

const user: ClientTimelineItem = { id: "user", turnId: "turn-1", kind: "user", text: "test", status: "completed" };
const pending: ClientTimelineItem = { id: "assistant", turnId: "turn-1", kind: "assistant", text: "", status: "in_progress" };
const provider: ClientTimelineItem = {
  id: "provider", kind: "activity", text: "Verifying model attestation and secure endpoint binding",
  raw: { kind: "notice", metadata: { code: "provider_status" } },
};

function render(timeline: ClientTimelineItem[]) {
  return renderToStaticMarkup(createElement(TimelineView, { session: session(timeline) }));
}

test("pending response followed by status notices renders exactly one Generating label", () => {
  const html = render([user, pending, provider, {
    id: "security", kind: "activity", text: "",
    raw: { kind: "notice", metadata: { code: "security_status" } },
  }]);
  assert.equal(html.match(/Generating/g)?.length, 1);
  assert.doesNotMatch(html, /Verifying model attestation/);
  const badge = renderToStaticMarkup(createElement(TeeVerificationBadge, {
    session: session([]), modelId: "test-model", webEnabled: false, onVerify: () => {},
  }));
  assert.match(badge, /Verifying reply/);
});

test("fallback appears once before the assistant placeholder arrives", () => {
  assert.equal(render([user, provider]).match(/Generating/g)?.length, 1);
});

test("a visible warning after a pending response does not add another indicator", () => {
  const html = render([user, pending, { id: "warning", kind: "activity", text: "Retrying request" }]);
  assert.equal(html.match(/Generating/g)?.length, 1);
  assert.match(html, /Retrying request/);
});

test("streamed text does not gain a second empty response after provider status", () => {
  const html = render([user, { ...pending, text: "Partial answer" }, provider]);
  assert.match(html, /Partial answer/);
  assert.doesNotMatch(html, /Generating|Verifying model attestation/);
});

test("a previous turn cannot suppress the current turn's loading indicator", () => {
  const html = render([{ ...pending, turnId: "older-turn", text: "Previous answer" }, user, provider]);
  assert.equal(html.match(/Generating/g)?.length, 1);
});

test("optimistic new prompts show their own single loading indicator", () => {
  const html = renderToStaticMarkup(createElement(TimelineView, {
    session: session([{ ...pending, text: "Previous answer", status: "completed" }]),
    pendingUserMessage: { id: "optimistic-user", sessionId: "thread-1", text: "Next question" },
  }));
  assert.equal(html.match(/Generating/g)?.length, 1);
  assert.match(html, /Next question/);
});

test("settled replies do not show a generation indicator", () => {
  const current = session([user, { ...pending, text: "Done", status: "completed", terminalVerified: true }, provider]);
  current.running = false;
  const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
  assert.doesNotMatch(html, /Generating|Verifying model attestation/);
});

test("cancelled and interrupted partial replies remain copyable without an error banner or receipt badge", () => {
  for (const status of ["cancelled", "interrupted"]) {
    const current = session([user, { ...pending, text: "Keep this partial answer", status, terminalVerified: false }]);
    current.running = false;
    const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
    assert.match(html, /Keep this partial answer/);
    assert.doesNotMatch(html, /before verified completion|unverified|cannot be copied|color-danger|Generating|E2EE/);
    const copyButton = html.match(/<button[^>]*aria-label="Copy"[^>]*>/)?.[0];
    assert.ok(copyButton);
    assert.doesNotMatch(copyButton, /\sdisabled(?:=|\s|>)/);
    assert.equal(current.timeline[1]?.status, status);
    assert.equal(current.timeline[1]?.terminalVerified, false);
  }
});

test("actual verification failures stay explicit and cannot be copied as completed responses", () => {
  const current = session([user, { ...pending, text: "Failed response", status: "failed", terminalVerified: false }]);
  current.running = false;
  const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
  assert.match(html, /This reply couldn&#x27;t be verified\. Try again\./);
  assert.match(html, /<button[^>]*disabled=""[^>]*aria-label="Copy"/);
  assert.doesNotMatch(html, />E2EE</);
});

test("stream failures show one friendly notice without changing completion status", () => {
  for (const detail of [
    "Tinfoil stream ended without authenticated completion",
    "relay stream ended without terminal completion",
    "provider stream ended before [DONE]",
    "Tinfoil response stalled",
    "Tinfoil response timed out",
    "Tinfoil request timed out",
    "Tinfoil reported an inference error",
  ]) {
    const current = session([
      user,
      { ...pending, text: "Partial reply", status: "failed", terminalVerified: false },
      { id: "failure", turnId: "turn-1", kind: "error", text: detail },
    ]);
    current.running = false;
    const before = structuredClone(current);
    const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
    assert.equal(html.match(/Try again\./g)?.length, 1);
    assert.match(html, /role="status"/);
    assert.match(html, /<details[^>]*>/);
    assert.doesNotMatch(html, /<details[^>]*\sopen|role="alert"|final response could not be verified|This reply couldn|color-danger|>E2EE</);
    assert.match(html, /<button[^>]*disabled=""[^>]*aria-label="Copy"/);
    assert.deepEqual(current, before);
  }
});

test("authentication failures retain one error notice instead of being described as network interruptions", () => {
  for (const detail of ["Tinfoil response authentication or transport failed", "Tinfoil encrypted exchange failed", "stream response identity changed"]) {
    const current = session([
      user, { ...pending, text: "Failed reply", status: "failed", terminalVerified: false },
      { id: "failure", turnId: "turn-1", kind: "error", text: detail },
    ]);
    current.running = false;
    const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
    assert.equal(html.match(/role="alert"/g)?.length, 1);
    assert.doesNotMatch(html, /reply ended early|role="status"|This reply couldn|>E2EE</);
    assert.match(html, /<button[^>]*disabled=""[^>]*aria-label="Copy"/);
    assert.ok(html.includes(detail));
  }
});

test("a different turn's error cannot hide a failed reply's fallback notice", () => {
  const current = session([
    { id: "earlier-error", turnId: "earlier-turn", kind: "error", text: "Tinfoil response timed out" },
    user, { ...pending, text: "Failed reply", status: "failed", terminalVerified: false },
  ]);
  current.running = false;
  const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
  assert.match(html, /This reply couldn&#x27;t be verified\. Try again\./);
  assert.equal(html.match(/role="alert"/g)?.length, 1);
});

test("reasoning and empty replies use the turn error as their single failure notice", () => {
  for (const text of ["", "Partial reply"]) {
    const current = session([
      user,
      { ...pending, text, status: "failed", terminalVerified: false },
      { id: "thought", kind: "reasoning", turnId: "turn-1", text: "Partial thought", status: "failed", terminalVerified: false },
      { id: "failure", turnId: "turn-1", kind: "error", text: "Tinfoil stream ended without authenticated completion" },
    ]);
    current.running = false;
    const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
    assert.equal(html.match(/The reply ended early\. Try again\./g)?.length, 1);
    assert.doesNotMatch(html, /final response could not be verified|This reply couldn|Generating|>E2EE</);
  }
});

function searchTool(id: string, status = "completed"): ClientTimelineItem {
  return { id, kind: "tool", turnId: "turn-1", text: "web_search", status, tool: {
    callId: id, title: "web_search", kind: "search", status,
    input: { query: id }, content: [], locations: [],
  } };
}

test("assistant text and compact tool rows stay interleaved, with only a final action bar", () => {
  const current = session([
    user, { ...pending, text: "Let me check the web:", status: "completed" },
    searchTool("current news"), searchTool("more news"),
    { ...pending, id: "final", text: "The current news is xyz.", status: "completed", terminalVerified: true },
  ]);
  current.running = false;
  const html = renderToStaticMarkup(createElement(TimelineView, { session: current }));
  assert.ok(html.indexOf("Let me check the web:") < html.indexOf("Searched the web"));
  assert.ok(html.indexOf('data-tool-call-id="more news"') < html.indexOf("The current news is xyz."));
  assert.equal(html.match(/aria-label="Copy"/g)?.length, 1);
  assert.equal(html.match(/data-timeline-kind="tool"/g)?.length, 2);
  assert.doesNotMatch(html, /Generating|web_search|gap-7/);
});

test("a tool-first response hides the empty earlier placeholder and owns the active indicator", () => {
  const html = render([user, pending, searchTool("news", "in_progress")]);
  assert.match(html, /Searching the web for/);
  assert.doesNotMatch(html, /Generating|aria-label="Copy"/);
});

test("completed tools followed by no new text show one trailing generation indicator", () => {
  const html = render([user, { ...pending, text: "Checking" }, searchTool("news")]);
  assert.equal(html.match(/Generating/g)?.length, 1);
  assert.ok(html.indexOf("Searched the web") < html.indexOf("Generating"));
});

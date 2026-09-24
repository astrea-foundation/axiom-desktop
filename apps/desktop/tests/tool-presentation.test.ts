import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { ToolActivity } from "@axiom/axiom-acp-client";
import { ToolCallCard } from "../src/renderer/src/components/ToolCallCard";
import { toolFailureReason, toolLabel, toolSearchSources } from "../src/renderer/src/components/toolPresentation";

const search: ToolActivity = {
  callId: "search", title: "web_search", kind: "search", status: "completed",
  input: { query: "current news" }, content: [], locations: [],
};

test("web tools use phase-aware TUI-style action and target labels", () => {
  assert.equal(toolLabel(search), "Searched the web for “current news”");
  assert.equal(toolLabel({ ...search, status: "in_progress" }), "Searching the web for “current news”");
  assert.equal(toolLabel({ ...search, status: "failed" }), "Search failed: Web search for “current news”");
  assert.equal(toolLabel({ ...search, title: "Searching the web for `current news` · web_search" }), "Searched the web for “current news”");
  assert.equal(toolLabel({ ...search, title: "fetch_url", input: { url: "https://user:password@example.com/story?token=secret#private" } }), "Fetched example.com/story");
  assert.equal(toolLabel({ ...search, title: "Read `src/main.rs`", name: "read_file" }), "Read src/main.rs");
});

const fetch: ToolActivity = {
  callId: "fetch", title: "fetch_url", name: "fetch_url", kind: "fetch", status: "failed",
  input: { url: "https://user:password@example.com/story?token=secret#private" }, content: [], locations: [],
};

test("native fetch failures become short, accurate reasons in both live and saved output formats", () => {
  for (const [error, reason] of [
    ["fetch returned HTTP 403 Forbidden", "Blocked by website"],
    ["fetch returned HTTP 401 Unauthorized", "Website requires sign-in"],
    ["fetch returned HTTP 404 Not Found", "Page not found"],
    ["fetch returned HTTP 410 Gone", "Page not found"],
    ["fetch returned HTTP 429 Too Many Requests", "Too many requests"],
    ["fetch returned HTTP 503 Service Unavailable", "Website unavailable"],
    ["fetch returned HTTP 504 Gateway Timeout", "Request timed out"],
    ["fetch failed: operation timed out", "Request timed out"],
    ["fetch body exceeds size limit", "Page too large"],
    ["fetch content type is not text-readable: application/pdf", "Unsupported page format"],
    ["too many redirects", "Too many redirects"],
    ["invalid fetch URL: relative URL without a base", "Invalid URL"],
    ["DNS lookup failed: name or service not known", "Website not found"],
    ["destination resolves to a non-public address", "Blocked for safety"],
    ["local network destinations are blocked", "Blocked for safety"],
    ["Web is off for this message", "Web is off"],
    ["fetch failed: error sending request", "Couldn’t fetch page"],
  ]) {
    for (const content of [
      [{ type: "text", text: `tool error: ${error}` }],
      [{ type: "content", content: { type: "text", text: `tool error: ${error}` } }],
    ]) {
      const tool = { ...fetch, content };
      assert.equal(toolFailureReason(tool), reason);
      assert.equal(toolLabel(tool), `${reason}: example.com/story`);
    }
  }
});

test("tool failure summaries distinguish cancellation, consent, and account/search limits", () => {
  assert.equal(toolFailureReason({ ...fetch, status: "cancelled" }), "Cancelled");
  assert.equal(toolFailureReason({ ...fetch, status: "interrupted" }), "Interrupted");
  assert.equal(toolFailureReason({ ...fetch, status: "completed", content: [{ text: "HTTP 403" }] }), null);
  assert.equal(toolFailureReason({ ...search, status: "failed", content: [{ text: "Search limit reached (60 per minute per account). Retry after 30 seconds." }] }), "Search limit reached");
  assert.equal(toolFailureReason({ ...search, status: "failed", content: [{ text: "Sign in to Axiom again to use web search." }] }), "Sign in to Axiom again");
  assert.equal(toolFailureReason({ ...search, status: "failed", content: [{ text: "Axiom web search is temporarily unavailable" }] }), "Search service unavailable");
  assert.equal(toolFailureReason({ ...fetch, content: [{ text: "Unknown error: Bearer confidential-value" }] }), "Couldn’t fetch page");
});

test("fetch rows and failed tools are plain summaries without expandable raw payloads", () => {
  for (const tool of [
    { ...fetch, status: "completed", content: [{ text: "RAW_PAGE_HTML_AND_SCRIPT" }] },
    { ...fetch, status: "in_progress" },
    { ...fetch, content: [{ text: "tool error: fetch returned HTTP 403 Forbidden" }] },
    { ...search, status: "failed", content: [{ text: "raw internal error" }] },
  ]) {
    const html = renderToStaticMarkup(createElement(ToolCallCard, { tool }));
    assert.doesNotMatch(html, /<button|<pre|aria-expanded|aria-controls|show details|RAW_PAGE_HTML|raw internal error|tool error:|password|token=secret/);
    if (tool.callId === "fetch" && tool.status === "failed") assert.match(html, /Blocked by website/);
  }
  const html = renderToStaticMarkup(createElement(ToolCallCard, { tool: search }));
  assert.match(html, /<button|aria-expanded="false"/);
});

test("tool labels bound untrusted text, remove controls, and redact recognizable credentials", () => {
  const label = toolLabel({ ...search, input: { query: "news\n\u202esecret axa_example_token " + "long".repeat(200) } });
  assert.doesNotMatch(label, /\n|\u202e|axa_example_token/);
  assert.match(label, /\[REDACTED\]/);
  assert.ok(Array.from(label).length <= 221);
});

test("expanded search details expose bounded source summaries, not executable URLs or HTML", () => {
  const result = { title: "<script>bad()</script> Article", url: "https://example.com/story", snippet: "Summary" };
  const content = [{ type: "content", content: { type: "text", text: JSON.stringify({ results: [
    result, { ...result, url: "javascript:alert(1)" }, { ...result, url: "https://u:p@example.com/" },
  ] }) } }];
  const sources = toolSearchSources({ ...search, content });
  assert.deepEqual(sources, [{ ...result, host: "example.com" }]);
  assert.equal(toolSearchSources({ ...search, status: "failed", content }), null);
  assert.equal(toolSearchSources({ ...search, content: [{ text: "invalid JSON" }] }), null);
  assert.deepEqual(toolSearchSources({ ...search, content: [{ text: '{"results":[]}' }] }), []);
});

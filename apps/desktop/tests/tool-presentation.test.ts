import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { ToolActivity } from "@axiom/axiom-acp-client";
import { ToolCallCard } from "../src/renderer/src/components/ToolCallCard";
import { imageRequestSummary, toolFailureReason, toolLabel, toolSearchSources, viewedImageId } from "../src/renderer/src/components/toolPresentation";

const search: ToolActivity = {
  callId: "search", title: "web_search", kind: "search", status: "completed",
  input: { query: "current news" }, content: [], locations: [],
};

test("image sampling shows percentages, delivery is indeterminate, and completion clears progress", () => {
  const image: ToolActivity = {callId: "image", name: "generate_image", title: "generate_image", kind: "other", status: "in_progress",
    input: {prompt: "A mug"}, content: [], locations: [], imageProgress: {stage: "generating", percent: 38}};
  assert.equal(toolLabel(image), "Generating image (38%)");
  assert.equal(toolLabel({...image, input: {image_id: "img_example"}}), "Editing image (38%)");
  const receiving: ToolActivity = {...image, imageProgress: {stage: "finishing", percent: 99}};
  assert.equal(toolLabel(receiving), "Receiving image…");
  const receivingHtml = renderToStaticMarkup(createElement(ToolCallCard, {tool: receiving}));
  assert.match(receivingHtml, /role="progressbar"/);
  assert.match(receivingHtml, /aria-valuetext="Receiving image"/);
  assert.doesNotMatch(receivingHtml, /aria-valuenow|99%|100%/);
  assert.equal(toolLabel({...image, status: "completed"}), "Generated image");
  const completed = renderToStaticMarkup(createElement(ToolCallCard, {tool: {...receiving, status: "completed"}}));
  assert.doesNotMatch(completed, /progressbar|100%/);
  const html = renderToStaticMarkup(createElement(ToolCallCard, {tool: image}));
  assert.match(html, /role="progressbar"/);
  assert.match(html, /aria-valuenow="38"/);
  assert.doesNotMatch(html, /step|38\s*(?:of|\/)\s*100/i);
  const failed = renderToStaticMarkup(createElement(ToolCallCard, {tool: {...receiving, status: "failed"}}));
  assert.doesNotMatch(failed, /progressbar|100%/);
});

test("the unified image tool labels generation and reference edits, including saved legacy edits", () => {
  const image: ToolActivity = { callId: "image", name: "generate_image", title: "generate_image", kind: "other",
    status: "completed", input: { prompt: "A blue mug" }, content: [], locations: [] };
  assert.equal(toolLabel(image), "Generated image");
  const edit = { ...image, input: { prompt: "Make it blue", image_id: `img_${"a".repeat(32)}` } };
  assert.equal(toolLabel(edit), "Edited image");
  assert.equal(toolLabel({ ...edit, status: "in_progress" }), "Editing image");
  assert.match(toolLabel({ ...edit, status: "failed" }), /^Image editing failed/);
  assert.equal(toolLabel({ ...edit, name: "edit_image", title: "edit_image" }), "Edited image");
});

test("image capacity failures identify a busy service, including saved connection-preparation wording", () => {
  for (const detail of ["AstreaAI is temporarily busy. Your request was not submitted; try again shortly.",
    "AstreaAI is busy preparing a secure connection. Your request was not submitted; try again shortly."]) {
    const tool: ToolActivity = { callId: "image", name: "generate", title: "generate", kind: "other", status: "failed",
      input: { prompt: "A mug" }, content: [{ type: "text", text: detail }], locations: [] };
    assert.equal(toolLabel(tool), "Image generation failed: Image service busy");
  }
});

test("image failures distinguish gateway rejection, verification failure and unknown causes", () => {
  const tool: ToolActivity = { callId: "image", name: "generate", title: "generate", kind: "other", status: "failed", content: [], locations: [] };
  for (const [error, label] of [
    ["Gateway completion failed", "Image generation failed: Gateway did not confirm completion"],
    ["The gateway could not complete the image request (REQUEST_FAILED).", "Image generation failed: Gateway did not confirm completion"],
    ["Axiom could not verify the final gateway response for this image.", "Image generation failed: Response verification failed"],
    ["Unknown failure with private details", "Image generation failed"],
  ]) {
    assert.equal(toolLabel({ ...tool, content: [{ type: "text", text: JSON.stringify({ error }) }] }), label);
  }
});

test("generate renders ordered references and preserves percentage progress", () => {
  const image: ToolActivity = {callId: "new-image", name: "generate", title: "generate", kind: "other", status: "in_progress",
    input: {prompt: "Use <image1>", reference_image_ids: ["img_" + "a".repeat(32)]}, content: [], locations: [],
    imageProgress: {stage: "generating", percent: 38}};
  assert.equal(toolLabel(image), "Editing image (38%)");
  assert.equal(toolLabel({...image, input: {prompt: "A blue mug", reference_image_ids: []}}), "Generating image (38%)");
  const html = renderToStaticMarkup(createElement(ToolCallCard, {tool: image}));
  assert.match(html, /aria-valuenow="38"/);
});

test("view_image names the inspected image and never shows raw tool output", () => {
  const id = `img_${"3e".repeat(16)}`;
  const view: ToolActivity = { callId: "view", name: "view_image", title: "view_image", kind: "other", status: "completed",
    input: { image_id: id }, locations: [],
    content: [{ type: "text", text: `Viewed image ${id}. The image is supplied as visual input after the tool responses.` }] };
  assert.equal(viewedImageId(view), id);
  assert.equal(toolLabel(view), `Viewed image ${id}`);
  assert.equal(toolLabel({ ...view, status: "in_progress" }), `Viewing image ${id}`);
  assert.equal(toolLabel({ ...view, input: { image_id: "../../etc/passwd" } }), "Viewed image");
  assert.match(toolLabel({ ...view, status: "failed" }), /^Image viewing failed/);
  const html = renderToStaticMarkup(createElement(ToolCallCard, { tool: view, threadId: "thread" }));
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /supplied as visual input|&quot;input&quot;|"output"/);
  const failed = renderToStaticMarkup(createElement(ToolCallCard, { tool: { ...view, status: "failed" } }));
  assert.doesNotMatch(failed, /aria-expanded/, "a failed view has no image to show");
});

test("image requests summarize prompt, aspect ratio and ordered references without raw arguments", () => {
  const first = `img_${"a".repeat(32)}`, second = `img_${"b".repeat(32)}`;
  const tool: ToolActivity = { callId: "image", name: "generate", title: "generate", kind: "other", status: "completed",
    input: { prompt: "Put <image1> on <image2>", reference_image_ids: [first, second, "not-an-id"], aspect_ratio: "16:9" },
    content: [], locations: [] };
  assert.deepEqual(imageRequestSummary(tool), { prompt: "Put <image1> on <image2>", aspectRatio: "16:9", references: [first, second] });
  assert.deepEqual(imageRequestSummary({ ...tool, input: { prompt: "A mug" } }), { prompt: "A mug", aspectRatio: null, references: [] });
  assert.deepEqual(imageRequestSummary({ ...tool, name: "generate_image", input: { prompt: "Blue", image_id: first, width: 1024, height: 768 } }),
    { prompt: "Blue", aspectRatio: "4:3", references: [first] });
  assert.equal(imageRequestSummary({ ...tool, input: { prompt: "Again", width: 1216, height: 1824 } })!.aspectRatio, "2:3");
  assert.equal(imageRequestSummary({ ...tool, input: {} }), null);
  assert.equal(imageRequestSummary({ ...tool, name: "web_search", input: { prompt: "x" } }), null);
  const failed = renderToStaticMarkup(createElement(ToolCallCard, { tool: { ...tool, status: "failed" } }));
  assert.match(failed, /aria-expanded="false"/, "a failed request keeps its prompt viewable");
});

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

test("failed commands show their exit status and keep diagnostics behind the disclosure", () => {
  const command = { ...search, name: "run_shell", kind: "execute", status: "failed",
    title: "Command failed: `lookup --token [REDACTED]`", input: { command: "lookup --token private-input" } };
  const result = JSON.stringify({ exit_code: 1, stdout: "other tool available", stderr: "HTTP 503 in fixture", truncated: false });
  for (const content of [[{ type: "text", text: result }], [{ type: "content", content: { type: "text", text: result } }]]) {
    const tool = { ...command, content };
    assert.equal(toolLabel(tool), "Command exited with code 1 · lookup --token [REDACTED]");
    const html = renderToStaticMarkup(createElement(ToolCallCard, { tool }));
    assert.match(html, /aria-expanded="false"/);
    assert.doesNotMatch(html, /private-input|other tool available|HTTP 503/);
  }
});

test("background failures distinguish a process timeout, a wait timeout and a nonzero exit", () => {
  const tool = { ...search, name: "background_process", title: "Failed to use background process", status: "failed" };
  const failure = (text: string) => ({ ...tool, content: [{ type: "text", text }] });
  assert.equal(toolLabel(failure("failed: tool error: process timed out after 120 seconds")), "Background task timed out after 120 seconds");
  assert.equal(toolLabel(failure("failed: process exited with code 7")), "Background task exited with code 7");
  assert.equal(toolLabel({ ...failure("tool error: background wait timed out after 5 seconds"), name: "background_wait" }), "Wait timed out after 5 seconds");
  assert.equal(toolLabel({ ...failure(JSON.stringify({state: "completed", outcome: { exit_code: 3 }})), name: "background_wait" }), "Background task exited with code 3");
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

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import type { AgentApi } from "../src/preload/agent-api";
import { AgentStateImages } from "../src/preload/agent-state-images";

// Execute the real preload and image helper with Electron IPC/document hooks replaced.
// This catches a bridge accidentally dropping the fourth argument or falling
// back to a pre-consent main process without opening an Electron window.
function preload(invoke: (channel: string, ...args: unknown[]) => Promise<unknown>): AgentApi {
  const source = readFileSync(new URL("../src/preload/agent-api.ts", import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const exports: { agentApi?: AgentApi } = {};
  runInNewContext(outputText, {
    exports,
    window: { addEventListener: () => {} },
    require: (name: string) => {
      if (name === "./agent-state-images") return { AgentStateImages };
      assert.equal(name, "electron");
      return { ipcRenderer: { invoke, on: () => {}, removeListener: () => {} } };
    },
  });
  return exports.agentApi!;
}

test("the real preload carries explicit Web choices only on the consent-aware IPC channel", async () => {
  const calls: unknown[][] = [];
  const api = preload(async (...args) => { calls.push(args); return {}; });
  assert.equal("prompt" in api, false);
  await api.promptWithWebConsent("thread", "question", "message-off", false);
  await api.promptWithWebConsent("thread", "question", "message-on", true);
  assert.deepEqual(calls, [
    ["agent:prompt-with-agent-context", "thread", "question", "message-off", false, 0, undefined],
    ["agent:prompt-with-agent-context", "thread", "question", "message-on", true, 0, undefined],
  ]);
});

test("the preload preserves captured Agent settings revisions", async () => {
  const calls: unknown[][] = [];
  const api = preload(async (...args) => { calls.push(args); return {}; });
  await api.promptWithWebConsent("thread", "question", "message", false, 7);
  assert.deepEqual(calls, [["agent:prompt-with-agent-context", "thread", "question", "message", false, 7, undefined]]);
});

test("gift redemption crosses the billing IPC boundary with its captured account", async () => {
  const calls: unknown[][] = [];
  const api = preload(async (...args) => { calls.push(args); return {}; });
  await api.redeemGiftCode("test-gift", "account-a");
  assert.deepEqual(calls, [["agent:redeem-gift-code", "test-gift", "account-a"]]);
});

test("individual image regeneration sends only conversation and stored call identities",async()=>{
  const calls:unknown[][]=[];
  const api=preload(async(...args)=>{calls.push(args);return {callId:"new-image"};});
  assert.deepEqual(await api.regenerateImage("thread","original-image"),{callId:"new-image"});
  assert.deepEqual(calls,[["agent:image-regenerate","thread","original-image"]]);
});

test("a new preload with an old main process fails before any legacy prompt is sent", async () => {
  let legacyPrompts = 0;
  const api = preload(async (channel) => {
    if (channel === "agent:prompt") { legacyPrompts++; return {}; }
    throw new Error("No handler registered");
  });
  await assert.rejects(api.promptWithWebConsent("thread", "question", "message", false), /No handler/);
  assert.equal(legacyPrompts, 0);
});

test("steering carries turn identity, idempotency identity and Web consent through the real preload", async () => {
  const calls: unknown[][] = [];
  const api = preload(async (...args) => { calls.push(args); return { turnId: "turn", clientItemId: "input" }; });
  const request = { threadId: "thread", expectedTurnId: "turn", clientItemId: "input", text: "New direction", webEnabled: false };
  await api.steer(request);
  assert.deepEqual(calls, [["agent:steer", request]]);
});

test("the preload carries MCP authority captured when the message was queued", async () => {
  const calls: unknown[][] = [];
  const api = preload(async (...args) => { calls.push(args); return {}; });
  await api.promptWithWebConsent("thread", "question", "message", false, 7, 12);
  assert.deepEqual(calls, [["agent:prompt-with-agent-context", "thread", "question", "message", false, 7, 12]]);
});

test("image creation has no per-message toggle in the preload", async () => {
  const calls: unknown[][] = [];
  const api = preload(async (...args) => { calls.push(args); return {}; });
  await api.promptWithWebConsent("thread", "draw a mug", "image-request", false, 7, 12);
  await api.promptWithAttachments("thread", "make it blue", "edit-request", false, 7, [], 12);
  await api.revisePrompt("thread", "draw a mug", "user-item", 3, false, 7, 12);
  assert.deepEqual(calls, [
    ["agent:prompt-with-agent-context", "thread", "draw a mug", "image-request", false, 7, 12],
    ["agent:prompt-with-attachments", "thread", "make it blue", "edit-request", false, 7, [], 12],
    ["agent:prompt-revise", "thread", "draw a mug", "user-item", 3, false, 7, 12],
  ]);
});

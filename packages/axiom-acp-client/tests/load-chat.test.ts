import "./verify-state-views.js";
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { AxiomAcpClient } from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const binary = process.env.AXIOMCLI_TEST_BINARY ?? resolve(here, "../../../target/debug", process.platform === "win32" ? "axiomcli.exe" : "axiomcli");

async function restarted() {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-load-chat-"));
  const options = { command: binary, args: ["acp", "--frontend", "desktop-chat"],
    env: { AXIOMCLI_TEST_RUNNER: "echo", XDG_CONFIG_HOME: resolve(root, "config"), XDG_DATA_HOME: resolve(root, "data") } };
  let client = new AxiomAcpClient(options);
  await client.initialize(); await client.bootstrapDesktop(); await client.initializeDesktopState();
  const created = await client.newChat();
  await client.prompt(created.sessionId, "saved before restart", "load-chat-1");
  const cwd = client.getState().sessions[created.sessionId]!.cwd;
  await client.close();
  client = new AxiomAcpClient(options);
  await client.initialize(); await client.bootstrapDesktop(); await client.initializeDesktopState();
  // Record every native request with its start and finish order.
  const log: string[] = [];
  const request = client.process.request.bind(client.process) as (...args: unknown[]) => Promise<unknown>;
  (client.process as { request: unknown }).request = async (method: string, ...rest: unknown[]) => {
    const params = rest[0] as { limit?: number } | undefined;
    const name = method === "_axiom/thread/timeline" && params?.limit === 1 ? "peek" : method;
    log.push(`>${name}`);
    try { return await request(method, ...rest); }
    finally { log.push(`<${name}`); }
  };
  return { client, threadId: created.sessionId, cwd, log };
}

test("opening a listed thread loads it directly, then reads its timeline and MCP selection together", async () => {
  const { client, threadId, cwd, log } = await restarted();
  try {
    assert.equal(client.getState().catalog.find(thread => thread.threadId === threadId)?.cwd, cwd);
    assert.equal(client.getState().sessions[threadId], undefined, "a listed thread is not presented as loaded");
    let emissions = 0;
    client.on("state", () => { emissions++; });
    await client.loadChat(threadId);
    assert.deepEqual(log.slice(0, 2), [">session/load", "<session/load"], "no workspace peek precedes the load");
    assert.deepEqual(log.slice(2, 4).sort(), [">_axiom/desktop/mcp", ">_axiom/thread/timeline"], "both reads start before either finishes");
    assert.equal(log.filter(entry => entry.startsWith(">")).length, 3);
    assert.ok(emissions <= 4, `one load published ${emissions} states`);
    const session = client.getState().sessions[threadId]!;
    assert.equal(session.cwd, cwd);
    assert.ok(session.timeline.some(item => item.kind === "user" && item.text === "saved before restart"));
    assert.ok(session.desktopMcp, "the MCP selection is loaded with the thread");
  } finally { await client.close(); }
});

test("a stale listed workspace is rejected by the native runtime and re-read once", async () => {
  const { client, threadId, cwd, log } = await restarted();
  try {
    client.state.setCatalog(client.getState().catalog.map(thread => thread.threadId === threadId ? { ...thread, cwd: resolve(cwd, "..", "moved-elsewhere") } : thread));
    await client.loadChat(threadId);
    assert.deepEqual(log.filter(entry => entry.startsWith(">")).slice(0, 3), [">session/load", ">peek", ">session/load"]);
    assert.equal(client.getState().sessions[threadId]!.cwd, cwd, "the thread opens with its saved workspace only");
    await assert.rejects(client.loadChat("00000000-0000-4000-8000-000000000000"), "an unknown thread still fails");
  } finally { await client.close(); }
});

test("an unlisted thread still reads its saved workspace first", async () => {
  const { client, threadId, cwd, log } = await restarted();
  try {
    client.state.setCatalog([]);
    await client.loadChat(threadId);
    assert.deepEqual(log.filter(entry => entry.startsWith(">")).slice(0, 2), [">peek", ">session/load"]);
    assert.equal(client.getState().sessions[threadId]!.cwd, cwd);
  } finally { await client.close(); }
});

// Development benchmark for the main → renderer agent-state path. Not shipped.
//
//   pnpm exec tsx scripts/bench-agent-state.ts            synthetic copy costs
//   pnpm exec tsx scripts/bench-agent-state.ts load-chat  one loadChat against the debug sidecar
//
// The synthetic run reports the cost of ONE state change (a streamed chunk in
// the newest thread) for a renderer that already holds the previous state.
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deserialize, serialize } from "node:v8";
import { AxiomAcpClient, AxiomStateStore, type ClientState, type StateChange } from "@axiom/axiom-acp-client";
import { stateSnapshot } from "../../../packages/axiom-acp-client/src/state-snapshot";
import { AgentStateTransport } from "../src/main/agent-state-transport";
import { AgentStateImages } from "../src/preload/agent-state-images";
import { applyStatePatch } from "../src/shared/state-patch";

const here = dirname(fileURLToPath(import.meta.url));
const words = "privacy follows the task from the question you ask to the work you delegate every model every call one verified encrypted path ".split(" ");
const text = (bytes: number, seed: number) => {
  let out = "";
  for (let i = seed; out.length < bytes; i++) out += words[i % words.length] + " ";
  return out.slice(0, bytes);
};

function session(id: string, items: number): ClientState["sessions"][string] {
  const timeline: ClientState["sessions"][string]["timeline"] = [];
  for (let i = 0; i < items; i++) {
    const kind = i % 5 === 0 ? "user" : i % 5 === 3 ? "tool" : "assistant";
    const content = text(kind === "user" ? 240 : kind === "tool" ? 900 : 1600, i);
    // Durable rows keep the authoritative item in `raw`, as fromDurable does.
    const raw = { id: `${id}-item-${i}`, threadId: id, turnId: `turn-${i >> 2}`, sequence: i + 1,
      kind: kind === "user" ? "user_message" : kind === "tool" ? "tool_call" : "assistant_message", status: "completed",
      content, metadata: kind === "tool" ? { name: "web_search", arguments: { query: text(80, i) }, content: [{ type: "text", text: content }] } : {},
      createdAt: "2026-11-15T00:00:00Z", updatedAt: "2026-11-15T00:00:00Z" };
    timeline.push({ id: raw.id, turnId: raw.turnId, sequence: raw.sequence, kind, text: content, status: "completed", raw,
      ...(kind === "tool" ? { tool: { callId: raw.id, name: "web_search", title: "web_search", kind: "other", status: "completed",
        input: { query: text(80, i) }, content: [{ type: "text", text: content }], locations: [] } } : {}) });
  }
  return { sessionId: id, title: `Thread ${id}`, cwd: "/tmp/chat", settings: { model: "model", thinkingLevel: "medium", permissionProfile: "web" },
    security: { state: "verified" } as never, contextUsage: null,
    requestUsage: Array.from({ length: Math.ceil(items / 4) }, (_, i) => ({ requestId: i.toString(16).padStart(32, "0"), turnId: `turn-${i}`,
      purpose: "conversation", modelId: "model", inputTokens: "7686", outputTokens: "353", costMicrousd: "2730", settled: true,
      state: "completed", startedAtMs: "1763164800000", finishedAtMs: "1763164814000" })) as never,
    modes: [], currentModeId: "web", configOptions: [], timeline, interactions: [], running: false, needsResync: false,
    threadRevision: items, lastTimelineSequence: items, lastMessageAt: null, lastUserMessageAt: null };
}

function synthetic(sessions: number, items: number, catalog = 300): ClientState {
  return {
    connected: true, runtimeInstanceId: "bench", lastSequence: 1,
    account: { revision: 1, state: "valid", account: { id: "bench-account", linkedMethods: [] } },
    sessions: Object.fromEntries(Array.from({ length: sessions }, (_, i) => [`thread-${i}`, session(`thread-${i}`, items)])),
    catalog: Array.from({ length: catalog }, (_, i) => ({ threadId: `thread-${i}`, title: `Thread ${i}`, cwd: "/tmp/chat", origin: "desktop",
      profile: "web", selectedModel: "model", thinkingLevel: "medium", lifecycle: "ready", archived: false, revision: 4,
      lastTimelineSequence: 12, createdAt: "2026-11-15T00:00:00Z", updatedAt: "2026-11-15T00:00:00Z" })) as never,
    collections: { revision: 0, collections: [] }, preferences: null, billing: null, diagnostic: "", error: null,
  };
}

const fixed = (value: number) => value.toFixed(value < 1 ? 3 : value < 10 ? 2 : 1).padStart(8);

function copyCosts(): void {
  console.log("One streamed-chunk state change, mean ms per stage (main: store + encode + serialize; renderer: deserialize + preload + bridge copy and page apply)");
  console.log("sessions items  stateMB |   store   encode   serial | deserial  preload   bridge |    total  packetKB");
  for (const [sessions, items] of [[1, 200], [5, 200], [5, 1000], [20, 200], [20, 1000]] as const) {
    const store = new AxiomStateStore();
    const live = synthetic(sessions, items);
    // Install the synthetic state as the store's own value so `snapshot()` and
    // change tracking run exactly as they do in the app.
    (store as unknown as { value: ClientState }).value = live;
    const active = `thread-${sessions - 1}`;
    let sequence = 0;
    const change = () => store.standardUpdate({ sessionId: active, update: { sessionUpdate: "agent_message_chunk",
      messageId: `assistant:bench-turn`, content: { type: "text", text: `chunk ${sequence++} ` } } });
    const transport = new AgentStateTransport();
    const images = new AgentStateImages({ create: () => "blob:bench", revoke: () => {} });
    const emitted: [ClientState, StateChange][] = [];
    store.on("change", (next: ClientState, hint: StateChange) => emitted.push([next, hint]));
    // Prime both ends with the full state first; measure only the next change.
    change(); store.flush();
    let page = structuredClone(images.accept(deserialize(serialize(transport.encode(...emitted.at(-1)!))) as never));
    let wire!: Buffer;
    const stage = { store: 0, encode: 0, serial: 0, deserial: 0, preload: 0, bridge: 0 };
    const rounds = 15;
    for (let round = 0; round < rounds; round++) {
      let start = performance.now(); change(); store.flush(); const [state, hint] = emitted.at(-1)!; stage.store += performance.now() - start;
      start = performance.now(); const packet = transport.encode(state, hint); stage.encode += performance.now() - start;
      start = performance.now(); wire = serialize(packet); stage.serial += performance.now() - start;
      start = performance.now(); const received = deserialize(wire); stage.deserial += performance.now() - start;
      start = performance.now(); const { update } = images.ingest(received as never); stage.preload += performance.now() - start;
      // contextBridge deep-copies what crosses into the page world; the page
      // then applies it to the state it already holds.
      start = performance.now();
      const crossed = structuredClone(update)!;
      if ("state" in crossed) page = crossed.state; else for (const patch of crossed.patches) page = applyStatePatch(page, patch);
      stage.bridge += performance.now() - start;
    }
    const each = Object.values(stage).map(value => value / rounds);
    const megabytes = serialize(stateSnapshot(live)).byteLength / 1024 / 1024;
    console.log(`${String(sessions).padStart(8)} ${String(items).padStart(5)} ${megabytes.toFixed(1).padStart(8)} | ${each.slice(0, 3).map(fixed).join(" ")} | ${each.slice(3).map(fixed).join(" ")} | ${fixed(each.reduce((a, b) => a + b, 0))} ${fixed(wire.byteLength / 1024)}`);
  }
}

async function loadChat(): Promise<void> {
  const binary = process.env.AXIOMCLI_TEST_BINARY ?? resolve(here, "../../../target/debug", process.platform === "win32" ? "axiomcli.exe" : "axiomcli");
  const root = await mkdtemp(resolve(tmpdir(), "axiom-bench-"));
  const home = resolve(root, "home"); await mkdir(home);
  await writeFile(resolve(home, "README.md"), "bench\n");
  const options = { command: binary, args: ["acp", "--frontend", "desktop-chat"], cwd: home,
    env: { AXIOMCLI_TEST_RUNNER: "echo", XDG_CONFIG_HOME: resolve(root, "config"), XDG_DATA_HOME: resolve(root, "data"), HOME: home } };
  const turns = Number(process.env.BENCH_TURNS ?? 40);
  let client = new AxiomAcpClient(options);
  await client.initialize(); await client.bootstrapDesktop(); await client.initializeDesktopState();
  const created = await client.newChat();
  for (let i = 0; i < turns; i++) await client.prompt(created.sessionId, `${text(400, i)} #${i}`, `bench-${i}`);
  await client.close();

  client = new AxiomAcpClient(options);
  await client.initialize(); await client.bootstrapDesktop(); await client.initializeDesktopState();
  let emissions = 0;
  client.on("state", () => { emissions++; });
  const requests: { method: string; started: number; ms?: number }[] = [];
  const request = client.process.request.bind(client.process);
  (client.process as { request: unknown }).request = async (method: string, ...rest: unknown[]) => {
    const entry: (typeof requests)[number] = { method, started: performance.now() };
    requests.push(entry);
    try { return await (request as (...args: unknown[]) => Promise<unknown>)(method, ...rest); }
    finally { entry.ms = performance.now() - entry.started; }
  };
  const start = performance.now();
  await client.loadChat(created.sessionId);
  const total = performance.now() - start;
  await new Promise(resolve => setTimeout(resolve, 50));
  const origin = requests[0]?.started ?? start;
  console.log(`loadChat of a ${turns}-turn thread (${client.getState().sessions[created.sessionId]?.timeline.length} timeline items), debug sidecar: ${total.toFixed(1)} ms, ${emissions} state emissions`);
  for (const entry of requests) console.log(`  +${(entry.started - origin).toFixed(1).padStart(6)} ms  ${entry.method.padEnd(28)} ${entry.ms?.toFixed(1)} ms`);
  await client.close();
}

if (process.argv[2] === "load-chat") await loadChat();
else copyCosts();

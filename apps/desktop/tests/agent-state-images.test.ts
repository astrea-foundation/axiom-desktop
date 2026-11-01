import assert from "node:assert/strict";
import test from "node:test";
import type { ClientState } from "@axiom/axiom-acp-client";
import { AgentStateTransport } from "../src/main/agent-state-transport";
import { AgentStateImages } from "../src/preload/agent-state-images";

// Synthetic header/payload for transport tests; native is responsible for
// complete authenticated PNG validation before these local presentation steps.
function png(width = 1536, height = 1536, bytes = 4 * 1024 * 1024): string {
  const raw = Buffer.alloc(bytes, 7);
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]).copy(raw);
  raw.writeUInt32BE(width, 16); raw.writeUInt32BE(height, 20);
  return raw.toString("base64");
}
function state(data: string, accountId = "one"): ClientState {
  const block = { type: "image", mimeType: "image/png", data };
  return {
    connected: true, runtimeInstanceId: "native-one", lastSequence: 1,
    account: { revision: 1, state: "valid", account: { id: accountId, linkedMethods: [] } },
    sessions: { thread: {
      sessionId: "thread", title: null, cwd: "", settings: null, security: null,
      modes: [], currentModeId: null, configOptions: [], interactions: [], running: false,
      needsResync: false, threadRevision: 1, lastTimelineSequence: 1,
      timeline: [{ id: "image", kind: "tool", text: "", raw: { metadata: { content: [block] } },
        tool: { callId: "image", name: "generate_image", title: "Image", kind: "other", status: "completed", input: {}, content: [block], locations: [] } }],
    } },
    catalog: [], collections: { revision: 0, collections: [] }, preferences: null,
    billing: null, diagnostic: "", error: null,
  };
}
function image(s: ClientState): Record<string, unknown> {
  return s.sessions.thread!.timeline[0]!.tool!.content[0] as Record<string, unknown>;
}
function resources() {
  const created: { url: string; bytes: number }[] = [], revoked: string[] = [];
  const api = { create: (blob: Blob) => {
    const url = `blob:local-${created.length}`; created.push({ url, bytes: blob.size }); return url;
  }, revoke: (url: string) => { revoked.push(url); } };
  return { created, revoked, api };
}

test("streaming state carries a multi-megabyte image once and keeps local URLs stable", () => {
  const native = state(png());
  const transport = new AgentStateTransport(), r = resources(), preload = new AgentStateImages(r.api);
  const first = transport.encode(native);
  assert.equal(first.images.length, 1, "raw and tool content share one payload");
  assert.ok(JSON.stringify(first.state).length < 3000, "state contains references, not duplicated base64");
  const initial = preload.accept(first);
  const url = image(initial).imageUrl;
  assert.match(String(url), /^blob:/);
  assert.equal(image(initial).data, undefined);
  for (let i = 0; i < 100; i++) {
    native.lastSequence++;
    const packet = transport.encode(native);
    assert.equal(packet.images.length, 0);
    assert.ok(JSON.stringify(packet).length < 4000);
    assert.equal(image(preload.accept(packet)).imageUrl, url);
  }
  assert.deepEqual(r.created.map(x => x.bytes), [4 * 1024 * 1024]);
  assert.deepEqual(r.revoked, []);
  assert.equal(image(native).data, png(), "presentation never mutates authoritative state");
});

test("reload and late bootstrap recover bytes without rolling back newer text", () => {
  const native = state(png(1024, 1024, 1024));
  const transport = new AgentStateTransport();
  transport.encode(native); // Event sent before the new preload subscribed.
  const full = transport.encode(native, true);
  native.lastSequence = 9;
  native.sessions.thread!.timeline[0]!.text = "newer";
  const newer = transport.encode(native);
  const r = resources(), preload = new AgentStateImages(r.api);
  assert.equal(image(preload.accept(newer)).imageUrl, undefined);
  const recovered = preload.accept(full);
  assert.equal(recovered.lastSequence, 9);
  assert.equal(recovered.sessions.thread!.timeline[0]!.text, "newer");
  assert.match(String(image(recovered).imageUrl), /^blob:/);
  const reloaded = new AgentStateImages(resources().api).accept(transport.encode(native, true));
  assert.match(String(image(reloaded).imageUrl), /^blob:/);
});

test("logout, account switching and stale replies cannot retain or restore old image URLs", () => {
  const transport = new AgentStateTransport(), r = resources(), preload = new AgentStateImages(r.api);
  const native = state(png(1024, 1024, 1024));
  const old = transport.encode(native, true);
  const oldUrl = image(preload.accept(old)).imageUrl;
  const signedOut = { ...native, account: { revision: 2, state: "signed_out" as const } };
  const logout = transport.encode(signedOut);
  assert.equal(logout.images.length, 0);
  assert.equal(image(preload.accept(logout)).imageUrl, undefined);
  assert.deepEqual(r.revoked, [oldUrl]);
  assert.equal(image(preload.accept(old)).imageUrl, undefined);
  const other = transport.encode(state(png(1024, 1024, 1024), "two"));
  assert.notEqual(other.context, old.context);
  const newUrl = image(preload.accept(other)).imageUrl;
  assert.notEqual(newUrl, oldUrl);
  assert.equal(preload.accept(old).account?.account?.id, "two");
  assert.equal(image(preload.accept(old)).imageUrl, newUrl);
  preload.dispose();
  assert.deepEqual(r.revoked, [oldUrl, newUrl]);
});

test("deleted images release resources and each window has independent opaque references", () => {
  const native = state(png(1024, 1024, 1024));
  const a = new AgentStateTransport(), b = new AgentStateTransport();
  const x = a.encode(native), y = b.encode(native);
  assert.notEqual(image(x.state).axiomImageAsset, image(y.state).axiomImageAsset);
  assert.equal(y.images.length, 1);
  const r = resources(), preload = new AgentStateImages(r.api);
  const url = image(preload.accept(x)).imageUrl;
  native.sessions.thread!.timeline = [];
  preload.accept(a.encode(native));
  assert.deepEqual(r.revoked, [url]);
});

test("disconnect and runtime replacement revoke resources even for the same account", () => {
  const native = state(png(1024, 1024, 1024));
  const transport = new AgentStateTransport(), r = resources(), preload = new AgentStateImages(r.api);
  const initial = transport.encode(native);
  const oldUrl = image(preload.accept(initial)).imageUrl;
  const offline = transport.encode({ ...native, connected: false });
  assert.equal(image(preload.accept(offline)).imageUrl, undefined);
  assert.deepEqual(r.revoked, [oldUrl]);
  const online = transport.encode(native);
  const nextUrl = image(preload.accept(online)).imageUrl;
  assert.notEqual(nextUrl, oldUrl);
  const restarted = transport.encode({ ...native, runtimeInstanceId: "native-two" });
  const restartedUrl = image(preload.accept(restarted)).imageUrl;
  assert.notEqual(restartedUrl, nextUrl);
  assert.equal(image(preload.accept(online)).imageUrl, restartedUrl);
  assert.throws(() => preload.accept({ ...restarted, context: "forged-context" }), /context changed/);
});

test("invalid geometry, noncanonical base64 and injected local URLs never become resources", () => {
  for (const data of [png(2048, 2048, 1024), png(1537, 1024, 1024), "not a png", png(1024, 1024, 1024) + "\n"]) {
    const packet = new AgentStateTransport().encode(state(data));
    assert.equal(packet.images.length, 0);
    assert.equal(image(packet.state).data, undefined);
    assert.equal(image(packet.state).axiomImageAsset, undefined);
  }
  const native = state("");
  Object.assign(image(native), { imageUrl: "blob:injected", axiomImageAsset: "another-account" });
  const result = new AgentStateImages(resources().api).accept(new AgentStateTransport().encode(native));
  assert.equal(image(result).imageUrl, undefined);
});

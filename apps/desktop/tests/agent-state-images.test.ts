import assert from "node:assert/strict";
import { createHash } from "node:crypto";
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

function lazyImages(count = 1, bytes = 1024) {
  const native = state("");
  const replies = new Map<string, { data: string; mimeType: string; width: number; height: number; sha256: string }>();
  native.sessions.thread!.timeline = Array.from({ length: count }, (_, index) => {
    const raw = Buffer.from(png(1024, 1024, bytes), "base64"); raw[31] = index;
    const data = raw.toString("base64"), sha256 = createHash("sha256").update(raw).digest("hex");
    const callId = `image-${index}`;
    replies.set(callId, { data, mimeType: "image/png", width: 1024, height: 1024, sha256 });
    const block = { type: "image", mimeType: "image/png", width: 1024, height: 1024, imageResource: { callId, sha256 } };
    return { id: callId, kind: "tool", text: "", raw: { metadata: { content: [block] } },
      tool: { callId, name: "generate", title: "Image", kind: "other", status: "completed", input: {}, content: [block], locations: [] } };
  });
  return { native, replies };
}

test("lazy image state transfers no bytes and coalesces on-demand reads into stable cached URLs", async () => {
  const { native, replies } = lazyImages(8, 1024 * 1024);
  const transport = new AgentStateTransport(), r = resources(), preload = new AgentStateImages(r.api);
  const packet = transport.encode(native, true);
  const visible = preload.accept(packet);
  const id = String(image(visible).assetId);
  assert.equal(packet.images.length, 0);
  assert.ok(JSON.stringify(packet).length < 12_000);
  assert.equal(r.created.length, 0, "opening a thread does not create or fetch image blobs");
  assert.equal(image(visible).imageResource, undefined, "only opaque document handles reach React");
  let calls = 0;
  const read = (context: string, asset: string) => transport.readImage(context, asset, async request => {
    calls++; assert.equal(request.threadId, "thread"); return replies.get(request.callId)!;
  });
  const [a, b] = await Promise.all([preload.load(id, read), preload.load(id, read)]);
  assert.equal(calls, 1);
  assert.equal(a.imageUrl, b.imageUrl);
  preload.release(id); preload.release(id);
  assert.equal((await preload.load(id, read)).imageUrl, a.imageUrl);
  assert.equal(calls, 1, "scrolling back to a cached image avoids another transfer");
  preload.release(id);
  const other = new AgentStateTransport(); other.encode(native);
  await assert.rejects(other.readImage(packet.context!, id, async () => { throw new Error("must not read"); }), /no longer available/);
  preload.dispose();
  const reloaded = new AgentStateImages(r.api);
  const snapshot = transport.encode(native, true);
  const reloadId = String(image(reloaded.accept(snapshot)).assetId);
  assert.equal(snapshot.images.length, 0, "document reload also carries metadata only");
  assert.equal(calls, 1);
  assert.notEqual((await reloaded.load(reloadId, read)).imageUrl, a.imageUrl);
  assert.equal(calls, 2, "a new document fetches only its requested original");
  reloaded.dispose();
});

test("lazy image reads reject stale account/runtime/deletion results before creating a URL", async () => {
  for (const change of ["account", "runtime", "delete"]) {
    const { native, replies } = lazyImages();
    const transport = new AgentStateTransport(), r = resources(), preload = new AgentStateImages(r.api);
    const packet = transport.encode(native);
    const id = String(image(preload.accept(packet)).assetId);
    let finish!: () => void;
    const gate = new Promise<void>(resolve => { finish = resolve; });
    const pending = preload.load(id, (context, asset) => transport.readImage(context, asset, async request => {
      await gate; return replies.get(request.callId)!;
    }));
    const rejected = assert.rejects(pending, /context changed/);
    if (change === "account") native.account!.account!.id = "two";
    if (change === "runtime") native.runtimeInstanceId = "restarted";
    if (change === "delete") native.sessions.thread!.timeline = [];
    preload.accept(transport.encode(native));
    finish(); await rejected;
    assert.deepEqual(r.created, [], change);
    assert.deepEqual(r.revoked, [], change);
  }
});

test("lazy image cache evicts idle images within its byte budget and pins displayed images", async () => {
  const { native, replies } = lazyImages(5);
  const transport = new AgentStateTransport(), r = resources(), preload = new AgentStateImages(r.api, 2048);
  const packet = transport.encode(native), visible = preload.accept(packet);
  const ids = visible.sessions.thread!.timeline.map(row => String((row.tool!.content[0] as Record<string, unknown>).assetId));
  const read = (context: string, id: string) => transport.readImage(context, id, async request => replies.get(request.callId)!);
  const pinned = await preload.load(ids[0]!, read);
  for (const id of ids.slice(1)) { await preload.load(id, read); preload.release(id); }
  assert.equal(r.revoked.length, 2);
  assert.ok(!r.revoked.includes(pinned.imageUrl), "an active viewer retains its original URL");
  assert.equal(r.created.length - r.revoked.length, 3, "one displayed image plus two cached originals");
  preload.release(ids[0]!);
  assert.equal(r.created.length - r.revoked.length, 2);
  preload.dispose();
  assert.equal(r.revoked.length, r.created.length);
});

test("on-demand image reads bound concurrency and reject mismatched content", async () => {
  const { native, replies } = lazyImages(5);
  const transport = new AgentStateTransport(), preload = new AgentStateImages(resources().api);
  const packet = transport.encode(native), visible = preload.accept(packet);
  const ids = visible.sessions.thread!.timeline.map(row => String((row.tool!.content[0] as Record<string, unknown>).assetId));
  const gates: (() => void)[] = [];
  let active = 0, maximum = 0;
  const reads = ids.map(id => preload.load(id, (context, asset) => transport.readImage(context, asset, async request => {
    active++; maximum = Math.max(maximum, active);
    await new Promise<void>(resolve => gates.push(resolve)); active--;
    return replies.get(request.callId)!;
  })));
  assert.equal(gates.length, 2);
  for (let index = 0; index < 5; index++) {
    gates[index]!(); await new Promise(resolve => setImmediate(resolve));
  }
  await Promise.all(reads);
  assert.equal(maximum, 2);
  await assert.rejects(transport.readImage(packet.context!, String(image(packet.state).axiomImageAsset), async request => ({ ...replies.get(request.callId)!, data: png() })), /identity differs|content differs/);
});

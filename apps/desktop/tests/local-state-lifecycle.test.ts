import assert from "node:assert/strict";
import test from "node:test";
import { LocalStateLifecycle } from "../src/main/local-state-lifecycle";
import { flushForUpdate, flushLocalState, registerUpdateFlush, releaseLocalStateFlush, releaseUpdateFlush, updateRestartPending } from "../src/renderer/src/updateFlush";

function deferred() {
  let resolve!: () => void, reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test("close and quit coalesce and cannot stop native before the renderer ACK", async () => {
  const gate = deferred(), calls: string[] = [];
  const lifecycle = new LocalStateLifecycle<string>({
    flush: async () => { calls.push("flush"); await gate.promise; }, release: () => { calls.push("release"); },
    close: window => { calls.push(`close:${window}`); }, stop: async () => { calls.push("stop"); },
    quit: () => { calls.push("quit"); }, failed: () => { calls.push("failed"); },
  });
  const first = lifecycle.close("window");
  assert.equal(lifecycle.close("window"), first);
  assert.equal(lifecycle.quit(), first);
  await tick(); assert.deepEqual(calls, ["flush"]);
  gate.resolve(); await first;
  assert.deepEqual(calls, ["flush", "stop", "quit", "release"]);
});

test("failed close leaves its window and native open, releases the input lock, and can retry", async () => {
  let fail = true;
  const calls: string[] = [];
  const lifecycle = new LocalStateLifecycle<string>({
    flush: async () => { if (fail) throw Error("keyring unavailable"); }, release: () => { calls.push("release"); },
    close: window => { calls.push(`close:${window}`); }, stop: async () => { calls.push("stop"); }, quit: () => { calls.push("quit"); },
    failed: error => { assert.match(String(error), /keyring/); calls.push("failed"); },
  });
  await lifecycle.close("window");
  assert.deepEqual(calls, ["failed", "release"]);
  fail = false; await lifecycle.close("window");
  assert.deepEqual(calls, ["failed", "release", "close:window", "release"]);
});

test("the last window can request quit reentrantly without another flush or deadlock", async () => {
  const calls: string[] = [];
  const lifecycle = new LocalStateLifecycle<string>({
    flush: async () => { calls.push("flush"); }, release: () => {},
    close: () => { calls.push("close"); void lifecycle.quit(); }, stop: async () => { calls.push("stop"); },
    quit: () => { calls.push("quit"); }, failed: error => { throw error; },
  });
  await lifecycle.close("window");
  assert.deepEqual(calls, ["flush", "close", "stop", "quit"]);
});

test("account mutation waits for old-owner ACK and failed flush never logs out", async () => {
  const gate = deferred(), calls: string[] = [];
  const lifecycle = new LocalStateLifecycle<string>({
    flush: () => gate.promise, release: () => { calls.push("release"); }, close: () => {}, stop: async () => {}, quit: () => {}, failed: () => {},
  });
  const logout = lifecycle.account(async () => { calls.push("logout"); return "signed-out"; });
  await tick(); assert.deepEqual(calls, []);
  gate.reject(Error("cannot save"));
  await assert.rejects(logout, /cannot save/);
  assert.deepEqual(calls, ["release"]);
});

test("closing cancels pending browser login before acquiring a new close barrier", async () => {
  const gate = deferred(), calls: string[] = [];
  const lifecycle = new LocalStateLifecycle<string>({
    flush: async () => { calls.push("flush"); }, release: () => { calls.push("release"); }, close: () => { calls.push("close"); },
    stop: async () => {}, quit: () => {}, failed: error => { throw error; },
  });
  const login = lifecycle.account(() => gate.promise, async () => { calls.push("cancel"); gate.reject(Error("cancelled")); });
  await tick();
  void lifecycle.close("window").catch(() => {});
  await assert.rejects(login, /cancelled/);
  await tick();
  assert.deepEqual(calls, ["flush", "cancel", "release", "flush", "close", "release"]);
});

test("local close locks are independent from update notifications and stale failures", async () => {
  const old = deferred();
  const unregister = registerUpdateFlush(() => old.promise);
  const pending = flushLocalState();
  releaseUpdateFlush(); assert.equal(updateRestartPending(), true);
  releaseLocalStateFlush(); unregister();
  await flushLocalState();
  old.reject(Error("stale flush failure")); await assert.rejects(pending);
  assert.equal(updateRestartPending(), true);
  releaseLocalStateFlush();
  await flushForUpdate(); releaseUpdateFlush(); assert.equal(updateRestartPending(), false);
});

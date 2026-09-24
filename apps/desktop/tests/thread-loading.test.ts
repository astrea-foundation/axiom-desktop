import assert from "node:assert/strict";
import test from "node:test";
import { ThreadLoadTracker } from "../src/renderer/src/threadLoading";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

test("state notifications cannot loop on a failed thread load; explicit retry works", async () => {
  const loads = new ThreadLoadTracker();
  let calls = 0;
  const request = async () => { calls += 1; throw new Error("thread not found"); };
  assert.equal((await loads.load("thread", request)).state, "failed");
  for (let count = 0; count < 30; count += 1) {
    assert.equal((await loads.load("thread", request)).state, "ignored");
  }
  assert.equal(calls, 1);
  assert.equal((await loads.load("thread", request, true)).state, "failed");
  assert.equal(calls, 2);
});

test("duplicate selection and reconciliation calls share one pending load", async () => {
  const loads = new ThreadLoadTracker();
  const pending = deferred();
  const first = loads.load("thread", () => pending.promise);
  assert.equal((await loads.load("thread", () => assert.fail("duplicate load"), true)).state, "ignored");
  pending.resolve();
  assert.equal((await first).state, "loaded");
  assert.equal((await loads.load("thread", () => assert.fail("duplicate load"))).state, "ignored");
});

test("deleting a thread suppresses late load success and state-driven reopening", async () => {
  const loads = new ThreadLoadTracker();
  const pending = deferred();
  const first = loads.load("thread", () => pending.promise);
  loads.block("thread");
  assert.equal((await loads.load("thread", () => assert.fail("deleted thread reopened"), true)).state, "ignored");
  pending.resolve();
  assert.equal((await first).state, "ignored");
  assert.equal(loads.isBlocked("thread"), true);
});

test("deleting a thread suppresses late load errors, including after deletion fails", async () => {
  const loads = new ThreadLoadTracker();
  const pending = deferred();
  const first = loads.load("thread", () => pending.promise);
  loads.block("thread");
  loads.unblock("thread");
  pending.reject(new Error("stale load failure"));
  assert.equal((await first).state, "ignored");
  assert.equal((await loads.load("thread", async () => {}, true)).state, "loaded");
});

test("deleting one thread does not block loading another", async () => {
  const loads = new ThreadLoadTracker();
  loads.block("deleted");
  assert.equal((await loads.load("other", async () => {})).state, "loaded");
});

test("account or runtime reset invalidates stale loads and permits fresh loading", async () => {
  const loads = new ThreadLoadTracker();
  const pending = deferred();
  const first = loads.load("thread", () => pending.promise);
  loads.block("deleted");
  loads.reset();
  const second = loads.load("thread", async () => {});
  pending.reject(new Error("old account"));
  assert.equal((await first).state, "ignored");
  assert.equal((await second).state, "loaded");
  assert.equal(loads.isBlocked("deleted"), false);
});

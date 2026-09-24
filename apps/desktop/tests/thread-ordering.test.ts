import assert from "node:assert/strict";
import test from "node:test";
import { compareThreadMessages, newestMessageAt, orderFoldersByMessages } from "../src/renderer/src/threadOrdering";
import type { Folder, Thread } from "../src/renderer/src/types";

const old = "2026-01-01T00:00:00.000Z";
const recent = "2026-01-02T00:00:00.000Z";
const latest = "2026-01-03T00:00:00.000Z";
const thread = (id: string, lastMessageAt: string | null, folderId: string | null = null): Thread => ({
  id, title: id, folderId, lastMessageAt, lastUserMessageAt: lastMessageAt, updatedAt: latest, status: "idle", messages: [],
});
const folder = (id: string): Folder => ({ id, name: id, collapsed: true });

test("threads sort by actual messages, not metadata, running state, or input catalog order", () => {
  const threads = [thread("old", old), thread("empty", null), thread("new", recent)];
  threads[0]!.status = "working";
  assert.deepEqual(threads.sort(compareThreadMessages).map((t) => t.id), ["new", "old", "empty"]);
});

test("equal timestamps and empty threads have a deterministic ID tie-break", () => {
  const threads = [thread("b", recent), thread("z", null), thread("a", recent), thread("c", null)];
  assert.deepEqual(threads.sort(compareThreadMessages).map((t) => t.id), ["a", "b", "c", "z"]);
});

test("folder ordering uses the newest member message and keeps empty/tied folders stable", () => {
  const folders = [folder("empty"), folder("a"), folder("b"), folder("tie")];
  const threads = [thread("a1", old, "a"), thread("b1", recent, "b"), thread("t1", recent, "tie"), thread("loose", latest)];
  assert.deepEqual(orderFoldersByMessages(folders, threads).map((f) => f.id), ["b", "tie", "a", "empty"]);
  threads.push(thread("a2", latest, "a"));
  assert.deepEqual(orderFoldersByMessages(folders, threads).map((f) => f.id), ["a", "b", "tie", "empty"]);
  assert.deepEqual(folders.map((f) => f.id), ["empty", "a", "b", "tie"], "sorting must not mutate persisted folder order");
});

test("moving/deleting the newest thread recalculates both affected folders", () => {
  const folders = [folder("a"), folder("b")];
  const threads = [thread("old", old, "a"), thread("new", recent, "a")];
  threads[1]!.folderId = "b";
  assert.deepEqual(orderFoldersByMessages(folders, threads).map((f) => f.id), ["b", "a"]);
  threads.pop();
  assert.deepEqual(orderFoldersByMessages(folders, threads).map((f) => f.id), ["a", "b"]);
});

test("live message metadata wins over an older catalog; missing dates never become now", () => {
  assert.equal(newestMessageAt(old, recent), recent);
  assert.equal(newestMessageAt(recent, old), recent);
  assert.equal(newestMessageAt(null, undefined, "bad-date"), null);
});


test("interleaved streams preserve order; a new user submission moves the thread", () => {
  const threads = [thread("first", old, "a"), thread("second", recent, "b")];
  const folders = [folder("a"), folder("b")];
  for (let index = 0; index < 20; index++) {
    threads[index % 2]!.lastMessageAt = new Date(Date.parse(latest) + index).toISOString();
    assert.deepEqual([...threads].sort(compareThreadMessages).map((t) => t.id), ["second", "first"]);
    assert.deepEqual(orderFoldersByMessages(folders, threads).map((f) => f.id), ["b", "a"]);
  }
  threads[0]!.lastUserMessageAt = latest;
  assert.deepEqual([...threads].sort(compareThreadMessages).map((t) => t.id), ["first", "second"]);
});

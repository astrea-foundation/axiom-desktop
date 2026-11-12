import assert from "node:assert/strict";
import test from "node:test";
import { stateSnapshot } from "../src/state-snapshot.js";

test("image snapshots isolate all mutable containers without serializing image strings", () => {
  const data = "large-image-content".repeat(200_000);
  const image = { type: "image", data, nested: [{ value: "original" }] };
  const state = { timeline: [{ tool: { content: [image] }, raw: { image } }] };
  const snapshot = stateSnapshot(state);
  assert.notEqual(snapshot.timeline, state.timeline);
  assert.equal(snapshot.timeline[0]!.tool.content[0], snapshot.timeline[0]!.raw.image);
  snapshot.timeline[0]!.raw.image.nested[0]!.value = "changed";
  snapshot.timeline[0]!.tool.content.push({ ...image, data: "replacement" });
  assert.equal(state.timeline[0]!.raw.image.nested[0]!.value, "original");
  assert.equal(state.timeline[0]!.tool.content.length, 1);
  assert.equal(stateSnapshot(state).timeline[0]!.raw.image.data, data);
});

test("snapshot preserves JSON prototype keys as data and supports repeated references", () => {
  const state = JSON.parse('{"__proto__":{"polluted":true},"constructor":{"value":1}}');
  state.self = state;
  const copy = stateSnapshot(state);
  assert.equal(Object.getPrototypeOf(copy), Object.prototype);
  assert.equal(Object.hasOwn(copy, "__proto__"), true);
  assert.equal(copy.__proto__.polluted, true);
  assert.equal(({} as { polluted?: boolean }).polluted, undefined);
  assert.equal(copy.self, copy);
  copy.constructor.value = 2;
  assert.equal(state.constructor.value, 1);
});

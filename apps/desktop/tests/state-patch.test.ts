import assert from "node:assert/strict";
import test from "node:test";
import { applyStatePatch, diffState, mapStatePatch, type StatePatch } from "../src/shared/state-patch";

const image = (value: object) => (value as { type?: unknown }).type === "image";

test("equal values need no patch and a patch keeps everything it does not name", () => {
  const base = { a: { deep: [1, 2, { x: 1 }] }, b: { keep: true }, list: [{ id: 1 }, { id: 2 }], gone: 1 };
  assert.equal(diffState(base, structuredClone(base)), undefined);
  const next = { a: { deep: [1, 2, { x: 2 }] }, b: { keep: true }, list: [{ id: 1 }, { id: 2 }, { id: 3 }], added: null };
  const patch = diffState(base, next)!;
  const applied = applyStatePatch(base, patch);
  assert.deepEqual(applied, next);
  assert.equal(applied.b, base.b, "an untouched object is the same object");
  assert.equal(applied.list[0], base.list[0]);
  assert.equal(applied.a.deep[2] === base.a.deep[2], false);
  assert.deepEqual(base.a.deep[2], { x: 1 }, "the base is never mutated");
  assert.equal("gone" in applied, false);
  assert.equal(JSON.stringify(patch).includes("keep"), false, "only the difference is described");
});

test("arrays shrink, grow and replace; undefined and type changes survive", () => {
  for (const [previous, next] of [
    [[1, 2, 3], [1]], [[], [undefined, null]], [[{ a: 1 }], [{ a: undefined }]], [{ a: 1 }, [1]], [[1], { 0: 1 }],
    [{ a: undefined }, {}], [{}, { a: undefined }], [null, { a: 1 }], [{ a: [1, [2, [3]]] }, { a: [1, [2, [4]]] }], ["a", "b"],
    [{ __proto__: null, a: 1 }, { a: 1, b: 2 }],
  ] as const) {
    const patch = diffState(previous, next);
    assert.ok(patch, JSON.stringify([previous, next]));
    assert.deepStrictEqual(applyStatePatch(previous, patch), next);
    // IPC and the context bridge both copy patches structurally.
    assert.deepStrictEqual(applyStatePatch(previous, structuredClone(patch)), next);
  }
  const own = JSON.parse('{"__proto__":{"polluted":true}}');
  const applied = applyStatePatch({}, diffState({}, own)!) as Record<string, unknown>;
  assert.equal(Object.hasOwn(applied, "__proto__"), true);
  assert.equal(({} as { polluted?: boolean }).polluted, undefined);
});

test("atomic values are replaced whole and replacement payloads can be mapped", () => {
  const previous = { content: [{ type: "image", axiomImageAsset: "one", width: 512 }, { type: "text", text: "a" }] };
  const next = { content: [{ type: "image", axiomImageAsset: "two", width: 512 }, { type: "text", text: "b" }] };
  const patch = diffState(previous, next, image)!;
  assert.deepEqual(patch, { k: "o", o: { content: { k: "a", n: 2, a: [
    [0, { k: "v", v: next.content[0] }], [1, { k: "o", o: { text: { k: "v", v: "b" } } }],
  ] } } });
  const seen: unknown[] = [];
  const mapped = mapStatePatch(patch, (value) => { seen.push(value); return typeof value === "string" ? value.toUpperCase() : { mapped: true }; });
  assert.deepEqual(seen, [next.content[0], "b"]);
  assert.deepEqual(applyStatePatch(previous, mapped), { content: [{ mapped: true }, { type: "text", text: "B" }] });
  assert.deepEqual(applyStatePatch(previous, patch, () => "revived"), { content: ["revived", { type: "text", text: "revived" }] });
});

test("a patch that does not fit its base is rejected instead of guessed", () => {
  const bad: [unknown, StatePatch][] = [
    [{ a: 1 }, { k: "a", n: 1, a: [] }], [[1], { k: "o", o: {} }], [[1], { k: "a", n: 2, a: [] }],
    [[1], { k: "a", n: 1, a: [[4, { k: "v", v: 1 }]] }], [{}, { k: "o", o: { missing: { k: "o", o: {} } } }],
    [{}, { k: "x" } as unknown as StatePatch],
  ];
  for (const [base, patch] of bad) assert.throws(() => applyStatePatch(base, patch), /State patch|Unsupported/, JSON.stringify(patch));
});

test("random edits round-trip and preserve identity of unchanged branches", () => {
  let seed = 0x5eed;
  const random = (limit: number) => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed % limit; };
  const value = (depth: number): unknown => {
    const kind = depth > 3 ? random(4) : random(7);
    if (kind === 0) return random(5);
    if (kind === 1) return ["alpha", "beta", ""][random(3)];
    if (kind === 2) return [null, true, undefined][random(3)];
    if (kind === 3) return { type: "image", axiomImageAsset: `image-${random(4)}`, width: 256 * (1 + random(2)) };
    if (kind === 4) return Array.from({ length: random(4) }, () => value(depth + 1));
    return Object.fromEntries(Array.from({ length: random(4) }, (_, index) => [`k${index}`, value(depth + 1)]));
  };
  const edit = (input: unknown, depth = 0): unknown => {
    if (random(6) === 0 || input === null || typeof input !== "object") return value(depth);
    if (Array.isArray(input)) {
      const copy = input.slice();
      const action = random(3);
      if (action === 0 || !copy.length) copy.push(value(depth + 1));
      else if (action === 1) copy.splice(random(copy.length), 1);
      else { const index = random(copy.length); copy[index] = edit(copy[index], depth + 1); }
      return copy;
    }
    const copy = { ...(input as Record<string, unknown>) };
    const keys = Object.keys(copy);
    const action = random(3);
    if (action === 0 || !keys.length) copy[`k${random(6)}`] = value(depth + 1);
    else if (action === 1) delete copy[keys[random(keys.length)]!];
    else { const key = keys[random(keys.length)]!; copy[key] = edit(copy[key], depth + 1); }
    return copy;
  };
  for (let round = 0; round < 400; round++) {
    let current: unknown = { root: value(0), sibling: { untouched: [1, 2, 3] } };
    for (let step = 0; step < 12; step++) {
      const next = { root: edit((current as { root: unknown }).root), sibling: structuredClone((current as { sibling: unknown }).sibling) };
      const patch = diffState(current, next, image);
      const applied = patch ? applyStatePatch(current, patch) : current;
      assert.deepStrictEqual(applied, next);
      assert.equal((applied as { sibling: unknown }).sibling, (current as { sibling: unknown }).sibling);
      if (patch) assert.deepStrictEqual(applyStatePatch(current, structuredClone(patch)), next);
      current = applied;
    }
  }
});

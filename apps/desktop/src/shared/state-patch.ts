/**
 * Structural patches for plain JSON-like state (objects, arrays, primitives).
 *
 * A patch names only what differs, so applying it keeps every untouched object
 * and array identical (`===`) to the base. Patches are data: they are sent over
 * IPC and the context bridge, and applying one never mutates its base.
 */
export type StatePatch =
  /** Replace the value. An absent `v` replaces it with undefined. */
  | { k: "v"; v?: unknown }
  /** Patch an object: `o` holds changed or added keys, `d` removed keys. */
  | { k: "o"; o: Record<string, StatePatch>; d?: string[] }
  /** Patch an array to length `n`: `a` holds changed or added indices. */
  | { k: "a"; a: [number, StatePatch][]; n: number };

const plain = (value: unknown): value is Record<string, unknown> => {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
};

/**
 * Describe how to turn `previous` into `next`, or return undefined when they
 * are structurally equal. Values for which `atomic` is true are never patched
 * in part: any difference replaces the whole value.
 */
export function diffState(previous: unknown, next: unknown, atomic?: (value: object) => boolean): StatePatch | undefined {
  if (Object.is(previous, next)) return undefined;
  if (Array.isArray(previous) && Array.isArray(next)) {
    const changed: [number, StatePatch][] = [];
    for (let index = 0; index < next.length; index++) {
      if (index >= previous.length) { changed.push([index, { k: "v", v: next[index] }]); continue; }
      const patch = diffState(previous[index], next[index], atomic);
      if (patch) changed.push([index, patch]);
    }
    return changed.length || previous.length !== next.length ? { k: "a", a: changed, n: next.length } : undefined;
  }
  if (plain(previous) && plain(next)) {
    const whole = atomic && (atomic(previous) || atomic(next));
    const set: Record<string, StatePatch> = {};
    let changed = false;
    const removed: string[] = [];
    for (const key of Object.keys(next)) {
      const patch: StatePatch | undefined = Object.hasOwn(previous, key) ? diffState(previous[key], next[key], atomic) : { k: "v", v: next[key] };
      if (!patch) continue;
      if (whole) return { k: "v", v: next };
      Object.defineProperty(set, key, { value: patch, enumerable: true, writable: true, configurable: true });
      changed = true;
    }
    for (const key of Object.keys(previous)) if (!Object.hasOwn(next, key)) removed.push(key);
    if (removed.length && whole) return { k: "v", v: next };
    if (!changed && !removed.length) return undefined;
    return removed.length ? { k: "o", o: set, d: removed } : { k: "o", o: set };
  }
  return { k: "v", v: next };
}

/**
 * Apply a patch without mutating `base`. `revive` maps every value the patch
 * introduces (the payload of a replacement) before it is stored.
 */
export function applyStatePatch<T>(base: T, patch: StatePatch, revive?: (value: unknown) => unknown): T {
  if (patch.k === "v") return (revive ? revive(patch.v) : patch.v) as T;
  if (patch.k === "a") {
    if (!Array.isArray(base)) throw new Error("State patch expected an array");
    const next = base.slice(0, patch.n);
    for (const [index, item] of patch.a) {
      if (!Number.isSafeInteger(index) || index < 0 || index >= patch.n) throw new Error("State patch index is out of range");
      next[index] = applyStatePatch(item.k === "v" ? undefined : base[index], item, revive);
    }
    if (next.length !== patch.n) throw new Error("State patch left an incomplete array");
    return next as T;
  }
  if (patch.k !== "o" || !plain(patch.o)) throw new Error("Unsupported state patch");
  if (!plain(base)) throw new Error("State patch expected an object");
  const next: Record<string, unknown> = {};
  const removed = patch.d ? new Set(patch.d) : undefined;
  const define = (key: string, value: unknown) => Object.defineProperty(next, key, { value, enumerable: true, writable: true, configurable: true });
  for (const key of Object.keys(base)) {
    if (removed?.has(key)) continue;
    const item = Object.hasOwn(patch.o, key) ? patch.o[key] : undefined;
    define(key, item ? applyStatePatch(base[key], item, revive) : base[key]);
  }
  for (const key of Object.keys(patch.o)) {
    if (!Object.hasOwn(base, key)) define(key, applyStatePatch(undefined, patch.o[key]!, revive));
  }
  return next as T;
}

/** Rewrite every replacement payload in a patch; the patch shape is unchanged. */
export function mapStatePatch(patch: StatePatch, map: (value: unknown) => unknown): StatePatch {
  if (patch.k === "v") return { k: "v", v: map(patch.v) };
  if (patch.k === "a") return { k: "a", a: patch.a.map(([index, item]) => [index, mapStatePatch(item, map)]), n: patch.n };
  const set: Record<string, StatePatch> = {};
  for (const key of Object.keys(patch.o)) {
    Object.defineProperty(set, key, { value: mapStatePatch(patch.o[key]!, map), enumerable: true, writable: true, configurable: true });
  }
  return patch.d ? { k: "o", o: set, d: patch.d } : { k: "o", o: set };
}

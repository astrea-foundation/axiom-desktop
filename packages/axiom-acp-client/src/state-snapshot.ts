/** Copy mutable JSON containers without serializing immutable, multi-megabyte
 * image strings on every token. Snapshots still own every mutable container. */
export function stateSnapshot<T>(value: T, seen = new WeakMap<object, unknown>()): T {
  if (value === null || typeof value !== "object") return value;
  const previous = seen.get(value);
  if (previous) return previous as T;
  if (Array.isArray(value)) {
    const copy: unknown[] = [];
    seen.set(value, copy);
    for (const item of value) copy.push(stateSnapshot(item, seen));
    return copy as T;
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return structuredClone(value);
  const copy = Object.create(prototype) as Record<string, unknown>;
  seen.set(value, copy);
  for (const [key, item] of Object.entries(value)) {
    Object.defineProperty(copy, key, {
      value: stateSnapshot(item, seen), enumerable: true, writable: true, configurable: true,
    });
  }
  return copy as T;
}

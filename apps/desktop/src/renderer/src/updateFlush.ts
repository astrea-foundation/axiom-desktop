type FlushReason = "update" | "close" | "account";
const flushers = new Set<(reason: FlushReason) => Promise<void>>();
const locks = new Map<string, symbol>();
export function registerUpdateFlush(flush: (reason: FlushReason) => Promise<void>) { flushers.add(flush); return () => {flushers.delete(flush);}; }
export function updateRestartPending() { return locks.size > 0; }
export function releaseUpdateFlush() { locks.delete('update'); }
export function releaseLocalStateFlush() { locks.delete('local'); }
export function flushForUpdate() { return flush('update', 'update'); }
export function flushLocalState(reason: "close" | "account" = "close") { return flush('local', reason); }
async function flush(lock: string, reason: FlushReason) {
  const token = Symbol(lock);
  locks.set(lock, token);
  try { await Promise.all([...flushers].map(flush=>flush(reason))); }
  catch (error) { if (locks.get(lock) === token) locks.delete(lock); throw error; }
}

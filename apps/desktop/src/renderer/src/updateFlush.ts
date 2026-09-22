const flushers = new Set<() => Promise<void>>();
let restarting = false;
export function registerUpdateFlush(flush: () => Promise<void>) { flushers.add(flush); return () => {flushers.delete(flush);}; }
export function updateRestartPending() { return restarting; }
export function releaseUpdateFlush() { restarting = false; }
export async function flushForUpdate() {
  restarting = true;
  try { await Promise.all([...flushers].map(flush=>flush())); }
  catch (error) { restarting = false; throw error; }
}

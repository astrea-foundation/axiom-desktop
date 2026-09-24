import type { ConsentStorage } from "./webConsent";

const THREAD_WEB_VERSION = "axiom.thread-web.v1:";

function threadKey(accountId: string, threadId: string): string | null {
  // The welcome composer is a transient draft, never an account-wide default.
  return accountId && threadId && threadId !== "new"
    ? THREAD_WEB_VERSION + JSON.stringify([accountId, threadId]) : null;
}

export function readThreadWebAccess(storage: ConsentStorage | null, accountId: string, threadId: string): boolean {
  const key = threadKey(accountId, threadId);
  try { return key !== null && storage?.getItem(key) === "enabled"; }
  catch { return false; }
}

export function saveThreadWebAccess(storage: ConsentStorage | null, accountId: string, threadId: string, enabled: boolean): boolean {
  const key = threadKey(accountId, threadId);
  if (!key || !storage || typeof enabled !== "boolean") return false;
  try {
    storage.setItem(key, enabled ? "enabled" : "disabled");
    return true;
  } catch { return false; }
}

export function forgetThreadWebAccess(storage: (ConsentStorage & Partial<Pick<Storage, "removeItem">>) | null, accountId: string, threadId: string): void {
  const key = threadKey(accountId, threadId);
  if (!key) return;
  try { storage?.removeItem?.(key); } catch { /* Deleted thread IDs are never reused. */ }
}

import {registerUpdateFlush, updateRestartPending} from './updateFlush';
import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import { localPayloads } from "./localPayloads";
import { browserConsentStorage } from "./webConsent";

const key = (account: string | null, thread: string | null) =>
  `axiom.draft.v1:${encodeURIComponent(account ?? "anonymous")}:${encodeURIComponent(thread ?? "new")}`;

/** Drafts belong to an account and composer, including the new-thread composer.
 * Writes happen synchronously before navigation or any submission IPC. */
export function useAccountDraft(accountId: string | null, threadId: string | null = null) {
  const storage = browserConsentStorage();
  const drafts = useRef(new Map<string, string>());
  const versions = useRef(new Map<string, number>());
  const pendingSaves = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  useEffect(() => registerUpdateFlush(async () => {
    for (const timer of pendingSaves.current.values()) clearTimeout(timer);
    pendingSaves.current.clear();
    await Promise.all([...drafts.current].map(([scope,text]) => {
      const owner = decodeURIComponent(scope.split(':')[1]!);
      return localPayloads.put(owner, scope, {text, attachments: []});
    }));
  }), []);
  const previousAccount = useRef(accountId);
  const currentAccount = useRef(accountId);
  currentAccount.current = accountId;
  const [, changed] = useState(0);
  const read = (account: string | null, thread: string | null): string => {
    const scope = key(account, thread);
    if (!drafts.current.has(scope)) {
      try { drafts.current.set(scope, storage?.getItem(scope) ?? ""); }
      catch { drafts.current.set(scope, ""); }
    }
    return drafts.current.get(scope)!;
  };
  // Anonymous text may follow the first sign-in, but an authored account's text
  // is never rendered in another account (including before effects run).
  if (previousAccount.current === null && accountId !== null) {
    const anonymous = read(null, null);
    if (anonymous && !read(accountId, null)) {
      drafts.current.set(key(accountId, null), anonymous);
      try { storage?.setItem(key(accountId, null), anonymous); } catch { /* retained in memory */ }
    }
    clearTimeout(pendingSaves.current.get(key(null, null)));
    pendingSaves.current.delete(key(null, null));
    void localPayloads.remove("anonymous", key(null, null)).catch(() => undefined);
    drafts.current.set(key(null, null), "");
    try { storage?.setItem(key(null, null), ""); } catch { /* retained in memory */ }
  }
  previousAccount.current = accountId;
  const draft = read(accountId, threadId);
  useEffect(() => {
    const scope = key(accountId, threadId);
    const version = versions.current.get(scope) ?? 0;
    let active = true;
    // Synchronous draft storage can be newer than a debounced IndexedDB write
    // after an abrupt exit. Never replace it with that stale copy on launch.
    let immediate: string | null = null;
    try { immediate = storage?.getItem(scope) ?? null; } catch { /* use IndexedDB */ }
    void localPayloads.get(accountId ?? "anonymous", scope).then((saved) => {
      if (immediate !== null) return;
      if (!active || !saved || (versions.current.get(scope) ?? 0) !== version) return;
      drafts.current.set(scope, saved.text); changed((value) => value + 1);
    }).catch(() => undefined);
    return () => { active = false; };
  }, [accountId, threadId]);
  const writeDraft = useCallback((thread: string | null, next: SetStateAction<string>, requireDurable = false) => {
    if (updateRestartPending() || currentAccount.current !== accountId) return false;
    const scope = key(accountId, thread);
    let previous = drafts.current.get(scope);
    if (previous === undefined) {
      try { previous = storage?.getItem(scope) ?? ""; } catch { previous = ""; }
    }
    const text = typeof next === "function" ? next(previous) : next;
    if (requireDurable) {
      if (!storage) throw new Error("Draft storage is unavailable");
      storage.setItem(scope, text);
    }
    // Keep text in memory even if disk storage is unavailable. Submission has
    // its own mandatory durable outbox write before it can clear this draft.
    drafts.current.set(scope, text);
    versions.current.set(scope, (versions.current.get(scope) ?? 0) + 1);
    clearTimeout(pendingSaves.current.get(scope));
    pendingSaves.current.set(scope, setTimeout(() => {
      pendingSaves.current.delete(scope);
      void localPayloads.put(accountId ?? "anonymous", scope, { text, attachments: [] }).catch(() => undefined);
    }, text ? 250 : 0));
    try { storage?.setItem(scope, text); } catch { /* retained in memory */ }
    changed((value) => value + 1);
    return true;
  }, [accountId, storage]);
  const setDraft = useCallback((next: SetStateAction<string>) => writeDraft(threadId, next), [threadId, writeDraft]);
  const writeDraftDurable = useCallback(async (thread: string | null, text: string) => {
    if (updateRestartPending() || currentAccount.current !== accountId) return false;
    const scope = key(accountId, thread);
    clearTimeout(pendingSaves.current.get(scope));
    pendingSaves.current.delete(scope);
    await localPayloads.put(accountId ?? "anonymous", scope, { text, attachments: [] });
    if (updateRestartPending() || currentAccount.current !== accountId) return false;
    drafts.current.set(scope, text);
    versions.current.set(scope, (versions.current.get(scope) ?? 0) + 1);
    try { storage?.setItem(scope, text); } catch { /* IndexedDB committed. */ }
    changed((value) => value + 1);
    return true;
  }, [accountId, storage]);
  return [draft, setDraft, writeDraft, writeDraftDurable] as const;
}

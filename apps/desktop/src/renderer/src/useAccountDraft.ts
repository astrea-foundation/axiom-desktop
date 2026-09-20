import { useCallback, useRef, useState, type SetStateAction } from "react";
import { browserConsentStorage } from "./webConsent";

const key = (account: string | null, thread: string | null) =>
  `axiom.draft.v1:${encodeURIComponent(account ?? "anonymous")}:${encodeURIComponent(thread ?? "new")}`;

/** Drafts belong to an account and composer, including the new-thread composer.
 * Writes happen synchronously before navigation or any submission IPC. */
export function useAccountDraft(accountId: string | null, threadId: string | null = null) {
  const storage = browserConsentStorage();
  const drafts = useRef(new Map<string, string>());
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
    drafts.current.set(key(null, null), "");
    try { storage?.setItem(key(null, null), ""); } catch { /* retained in memory */ }
  }
  previousAccount.current = accountId;
  const draft = read(accountId, threadId);
  const writeDraft = useCallback((thread: string | null, next: SetStateAction<string>, requireDurable = false) => {
    if (currentAccount.current !== accountId) return false;
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
    try { storage?.setItem(scope, text); } catch { /* retained in memory */ }
    changed((value) => value + 1);
    return true;
  }, [accountId, storage]);
  const setDraft = useCallback((next: SetStateAction<string>) => writeDraft(threadId, next), [threadId, writeDraft]);
  return [draft, setDraft, writeDraft] as const;
}

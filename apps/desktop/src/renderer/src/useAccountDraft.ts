import { registerUpdateFlush, updateRestartPending } from "./updateFlush";
import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import { localPayloads } from "./localPayloads";

const key = (account: string | null, thread: string | null) =>
  `axiom.draft.v1:${encodeURIComponent(account ?? "anonymous")}:${encodeURIComponent(thread ?? "new")}`;

/** Text is immediately available in memory; durable writes go only to native
 * encrypted storage. Anonymous composers never touch browser persistence. */
export function useAccountDraft(accountId: string | null, threadId: string | null = null,
  report: (error: string | null) => void = () => {}, connectionRevision = 0) {
  const drafts = useRef(new Map<string, string>());
  const versions = useRef(new Map<string, number>());
  const pending = useRef(new Map<string, { account: string; text: string; timer: ReturnType<typeof setTimeout> }>());
  const current = useRef({ accountId, connectionRevision });
  current.current = { accountId, connectionRevision };
  const [, changed] = useState(0);
  const scope = key(accountId, threadId);
  const currentOwner = () => current.current.accountId === accountId && current.current.connectionRevision === connectionRevision;
  const failed = () => { if (currentOwner()) report("Your draft is kept in this window, but could not be saved. Keep Axiom open and retry."); };
  const save = useCallback((id: string, account: string, text: string) => localPayloads.put(account, id, { text, attachments: [] }), []);

  useEffect(() => registerUpdateFlush(async reason => {
    // An account transition saves its current owner before native closes that
    // context. Older dirty owners remain available to retry when signed back in.
    const writes = [...pending.current.entries()].filter(([, value]) => reason !== "account" || value.account === (current.current.accountId ?? "anonymous"));
    await Promise.all(writes.map(async ([id, value]) => {
      clearTimeout(value.timer);
      await save(id, value.account, value.text);
      if (pending.current.get(id) === value) pending.current.delete(id);
    }));
    await localPayloads.flush();
  }), [save]);

  useEffect(() => {
    let active = true;
    const version = versions.current.get(scope) ?? 0;
    void localPayloads.get(accountId ?? "anonymous", scope).then(async saved => {
      if (!active || !currentOwner() || (versions.current.get(scope) ?? 0) !== version) return;
      let text = saved?.text ?? "";
      const unsaved = pending.current.get(scope);
      if (unsaved) {
        text = unsaved.text;
        await save(scope, accountId ?? "anonymous", text);
        if (!active || !currentOwner() || (versions.current.get(scope) ?? 0) !== version) return;
        clearTimeout(unsaved.timer);
        if (pending.current.get(scope) === unsaved) pending.current.delete(scope);
      }
      const anonymous = key(null, null);
      // Adopt only an in-memory anonymous composer, after checking the owner's
      // encrypted draft. A failed commit leaves the anonymous draft intact.
      if (accountId && threadId === null && !text && drafts.current.get(anonymous)) {
        text = drafts.current.get(anonymous)!;
        await save(scope, accountId, text);
        if (!active || !currentOwner() || (versions.current.get(scope) ?? 0) !== version) return;
        const anonymousSave = pending.current.get(anonymous);
        if (anonymousSave) clearTimeout(anonymousSave.timer);
        pending.current.delete(anonymous);
        versions.current.set(anonymous, (versions.current.get(anonymous) ?? 0) + 1);
        drafts.current.set(anonymous, "");
        await localPayloads.remove("anonymous", anonymous);
      }
      if (!active || !currentOwner() || (versions.current.get(scope) ?? 0) !== version) return;
      drafts.current.set(scope, text); changed(value => value + 1);
    }).catch(() => { if (active) failed(); });
    return () => { active = false; };
  }, [accountId, threadId, connectionRevision, scope, save]);

  const writeDraft = useCallback((thread: string | null, next: SetStateAction<string>) => {
    if (updateRestartPending() || !currentOwner()) return false;
    const id = key(accountId, thread);
    const previous = drafts.current.get(id) ?? "";
    const text = typeof next === "function" ? next(previous) : next;
    drafts.current.set(id, text);
    versions.current.set(id, (versions.current.get(id) ?? 0) + 1);
    const old = pending.current.get(id);
    if (old) clearTimeout(old.timer);
    const entry = { account: accountId ?? "anonymous", text, timer: undefined as unknown as ReturnType<typeof setTimeout> };
    entry.timer = setTimeout(() => {
      if (!currentOwner()) return;
      void save(id, entry.account, text).then(() => {
        if (pending.current.get(id) === entry) pending.current.delete(id);
      }).catch(failed);
    }, text ? 250 : 0);
    pending.current.set(id, entry);
    changed(value => value + 1);
    return true;
  }, [accountId, connectionRevision, save]);
  const setDraft = useCallback((next: SetStateAction<string>) => writeDraft(threadId, next), [threadId, writeDraft]);
  const writeDraftDurable = useCallback(async (thread: string | null, next: SetStateAction<string>) => {
    if (updateRestartPending() || !currentOwner()) return false;
    const id = key(accountId, thread);
    const previous = drafts.current.get(id) ?? "";
    const text = typeof next === "function" ? next(previous) : next;
    const version = versions.current.get(id) ?? 0;
    const old = pending.current.get(id);
    if (old) clearTimeout(old.timer);
    // A close/account flush racing this durable replacement must capture the
    // replacement, not an older debounced value that could overwrite it later.
    const entry = { account: accountId ?? "anonymous", text, timer: undefined as unknown as ReturnType<typeof setTimeout> };
    pending.current.set(id, entry);
    await save(id, accountId ?? "anonymous", text);
    if (updateRestartPending() || !currentOwner() || (versions.current.get(id) ?? 0) !== version) return false;
    if (pending.current.get(id) === entry) pending.current.delete(id);
    drafts.current.set(id, text);
    versions.current.set(id, version + 1);
    changed(value => value + 1);
    return true;
  }, [accountId, connectionRevision, save]);
  return [drafts.current.get(scope) ?? "", setDraft, writeDraft, writeDraftDurable] as const;
}

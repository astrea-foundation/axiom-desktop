import { AttachmentDraftWrites } from "./attachmentDraftWrites";
import { registerUpdateFlush, updateRestartPending } from "./updateFlush";
import { useEffect, useRef, useState } from "react";
import { MAX_ATTACHMENTS, validatePrompt, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";
import { readAttachment } from "./attachments";
import { localPayloads } from "./localPayloads";

export function useAttachmentDraft(accountId: string | null, threadId: string | null, report: (error: string | null) => void, connectionRevision = 0) {
  const account = accountId ?? "anonymous";
  const currentAccount = useRef(account); currentAccount.current = account;
  const id = `draft-attachments:${threadId ?? "new"}`;
  const scope = JSON.stringify([account, id]);
  const context = JSON.stringify([scope, connectionRevision]);
  const currentScope = useRef(context); currentScope.current = context;
  const drafts = useRef(new Map<string, PromptAttachment[]>());
  const processing = useRef(new Set<string>());
  const versions = useRef(new Map<string, number>());
  const writes = useRef<AttachmentDraftWrites | null>(null);
  if (!writes.current) writes.current = new AttachmentDraftWrites(localPayloads);
  useEffect(() => registerUpdateFlush(async reason => {
    if (processing.current.size) throw new Error("Wait for attachments to finish preparing, then retry.");
    await writes.current!.flush(reason === "account" ? currentAccount.current : undefined);
  }), []);
  const [, changed] = useState(0);
  useEffect(() => {
    let active = true;
    void writes.current!.retry(account, id).catch(() => {
      if (active && currentScope.current === context) report("Attachments are kept in this window, but could not be saved. Keep Axiom open.");
    });
    if (!drafts.current.has(scope)) {
      void localPayloads.get(account, id).then((payload) => {
        if (drafts.current.has(scope)) return;
        const attachments = payload?.attachments ?? [];
        validatePrompt("", attachments, true);
        drafts.current.set(scope, attachments);
        if (active) changed((value) => value + 1);
      }).catch(() => { if (active) report("Saved attachments could not be loaded."); }).finally(() => {
        // A failed load stays unknown so reconnect retries it instead of
        // mistaking an unread encrypted draft for an empty composer.
        if (active) changed((value) => value + 1);
      });
    }
    return () => { active = false; };
  }, [scope, context, account, id, report]);
  const setAttachments = (attachments: PromptAttachment[]) => {
    if (updateRestartPending()) return;
    drafts.current.set(scope, attachments);
    versions.current.set(scope, (versions.current.get(scope) ?? 0) + 1);
    changed((value) => value + 1);
    // Outbox submission separately waits for mandatory durable storage.
    void writes.current!.write(account, id, attachments).catch(() => { if (currentScope.current === context) report("Attachments are kept in this window, but could not be saved. Keep Axiom open."); });
  };
  const addFiles = async (files: File[]) => {
    if (updateRestartPending()) return;
    if (processing.current.has(scope)) return;
    const previous = drafts.current.get(scope) ?? [];
    if (previous.length + files.length > MAX_ATTACHMENTS) { report("Attach at most 10 files per message."); return; }
    processing.current.add(scope); changed((value) => value + 1);
    try {
      const added: PromptAttachment[] = [];
      // Read sequentially to bound attachment memory.
      for (const file of files) added.push(await readAttachment(file));
      const next = [...(drafts.current.get(scope) ?? previous), ...added];
      validatePrompt("", next);
      setAttachments(next);
    } catch (error) { if (currentScope.current === context) report(error instanceof Error ? error.message : "The file could not be read."); }
    finally { processing.current.delete(scope); changed((value) => value + 1); }
  };
  const setAttachmentsDurable = async (attachments: PromptAttachment[]) => {
    const version = versions.current.get(scope) ?? 0;
    await writes.current!.write(account, id, attachments);
    if (currentScope.current !== context || (versions.current.get(scope) ?? 0) !== version) return false;
    drafts.current.set(scope, attachments); versions.current.set(scope, version + 1); changed((value) => value + 1); return true;
  };
  const clearAttachments = (expected: PromptAttachment[]) => {
    if (drafts.current.get(scope) === expected) setAttachments([]);
  };
  return { clearAttachments, setAttachmentsDurable, attachments: drafts.current.get(scope) ?? [], setAttachments, addFiles, attachmentsBusy: processing.current.has(scope) || !drafts.current.has(scope), attachmentScope: scope };
}

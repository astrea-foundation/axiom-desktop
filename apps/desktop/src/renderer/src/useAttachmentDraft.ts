import { useEffect, useRef, useState } from "react";
import { MAX_ATTACHMENTS, validatePrompt, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";
import { readAttachment } from "./attachments";
import { localPayloads } from "./localPayloads";

export function useAttachmentDraft(accountId: string | null, threadId: string | null, report: (error: string | null) => void) {
  const account = accountId ?? "anonymous";
  const id = `draft-attachments:${threadId ?? "new"}`;
  const scope = JSON.stringify([account, id]);
  const currentScope = useRef(scope); currentScope.current = scope;
  const drafts = useRef(new Map<string, PromptAttachment[]>());
  const processing = useRef(new Set<string>());
  const [, changed] = useState(0);
  useEffect(() => {
    let active = true;
    if (!drafts.current.has(scope)) {
      void localPayloads.get(account, id).then((payload) => {
        if (!payload || drafts.current.has(scope)) return;
        validatePrompt("", payload.attachments, true);
        drafts.current.set(scope, payload.attachments);
        if (active) changed((value) => value + 1);
      }).catch(() => { if (active) report("Saved attachments could not be loaded."); }).finally(() => {
        if (!drafts.current.has(scope)) drafts.current.set(scope, []);
        if (active) changed((value) => value + 1);
      });
    }
    return () => { active = false; };
  }, [scope, account, id, report]);
  const setAttachments = (attachments: PromptAttachment[]) => {
    drafts.current.set(scope, attachments);
    changed((value) => value + 1);
    // Outbox submission separately waits for mandatory durable storage.
    void localPayloads.put(account, id, { text: "", attachments }).catch(() => { if (currentScope.current === scope) report("Attachments are kept in this window, but could not be saved. Keep Axiom open."); });
  };
  const addFiles = async (files: File[]) => {
    if (processing.current.has(scope)) return;
    const previous = drafts.current.get(scope) ?? [];
    if (previous.length + files.length > MAX_ATTACHMENTS) { report("Attach at most 8 files per message."); return; }
    processing.current.add(scope); changed((value) => value + 1);
    try {
      const added: PromptAttachment[] = [];
      // Read sequentially to bound attachment memory.
      for (const file of files) added.push(await readAttachment(file));
      const next = [...previous, ...added];
      validatePrompt("", next);
      setAttachments(next);
    } catch (error) { if (currentScope.current === scope) report(error instanceof Error ? error.message : "The file could not be read."); }
    finally { processing.current.delete(scope); changed((value) => value + 1); }
  };
  const setAttachmentsDurable = async (attachments: PromptAttachment[]) => {
    await localPayloads.put(account, id, { text: "", attachments });
    if (currentScope.current !== scope) return false;
    drafts.current.set(scope, attachments); changed((value) => value + 1); return true;
  };
  const clearAttachments = (expected: PromptAttachment[]) => {
    if (drafts.current.get(scope) === expected) setAttachments([]);
  };
  return { clearAttachments, setAttachmentsDurable, attachments: drafts.current.get(scope) ?? [], setAttachments, addFiles, attachmentsBusy: processing.current.has(scope) || !drafts.current.has(scope), attachmentScope: scope };
}

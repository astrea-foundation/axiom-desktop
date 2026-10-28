import { useCallback, useEffect, useRef, useState } from "react";
import { browserConsentStorage, rememberWebWarning, skipWebWarning } from "./webConsent";
import { forgetThreadWebAccess, readThreadWebAccess, saveThreadWebAccess } from "./webAccessStorage";

type WebState = { owner: string | null; enabled: Record<string, boolean>; warning: string | null; error: string | null };

/** Account-scoped, on-device per-chat choices; welcome drafts still start off. */
export function useWebAccess(accountId: string | null, runtimeId: string | null, connected: boolean, scope: string) {
  const owner = connected && accountId ? JSON.stringify([accountId, runtimeId]) : null;
  const [state, setState] = useState<WebState>({ owner: null, enabled: {}, warning: null, error: null });
  const current = useRef({ owner, scope });
  current.current = { owner, scope };
  const enabled = owner !== null && state.owner === owner && state.enabled[scope] === true;
  const warning = owner !== null && state.owner === owner && state.warning === scope;

  useEffect(() => {
    const restored = owner && accountId && scope !== "new"
      ? readThreadWebAccess(browserConsentStorage(), accountId, scope) : false;
    setState((previous) => ({
      owner,
      enabled: {
        ...(previous.owner === owner ? previous.enabled : {}),
        [scope]: previous.owner === owner && Object.hasOwn(previous.enabled, scope)
          ? previous.enabled[scope] === true : restored,
      },
      warning: null, error: null,
    }));
  }, [owner, accountId, scope]);

  const change = (next: boolean) => {
    if (!owner || !accountId || current.current.owner !== owner || current.current.scope !== scope) return false;
    const saved = scope === "new" || saveThreadWebAccess(browserConsentStorage(), accountId, scope, next);
    setState((previous) => ({
      owner, enabled: { ...(previous.owner === owner ? previous.enabled : {}), [scope]: saved && next },
      warning: null,
      error: saved ? null : next
        ? "Web could not be saved and remains off. Please try again."
        : "Web is off, but could not be saved. It may turn back on after restarting.",
    }));
    return saved;
  };

  const toggle = () => {
    if (!owner || !accountId || current.current.owner !== owner || current.current.scope !== scope) return;
    const needsWarning = !enabled && !skipWebWarning(browserConsentStorage(), accountId);
    if (!needsWarning) { change(!enabled); return; }
    setState((previous) => ({
      owner, enabled: { ...(previous.owner === owner ? previous.enabled : {}), [scope]: false },
      warning: scope, error: null,
    }));
  };
  const confirm = (remember: boolean) => {
    if (!warning || !accountId || current.current.owner !== owner || current.current.scope !== scope) return;
    if (change(true) && remember) rememberWebWarning(browserConsentStorage(), accountId);
  };
  const cancel = () => setState((previous) => ({ ...previous, warning: null }));
  const resetNew = useCallback(() => {
    setState((previous) => ({ ...previous, enabled: { ...previous.enabled, new: false }, warning: null, error: null }));
  }, []);
  const adoptThread = (threadId: string, capturedEnabled = enabled, resetDraft = true) => {
    if (!owner || !accountId || current.current.owner !== owner) {
      throw new Error("The account or connection changed before the message could be sent. Please try again.");
    }
    if (!saveThreadWebAccess(browserConsentStorage(), accountId, threadId, capturedEnabled)) {
      throw new Error("The chat's Web setting could not be saved. No message was sent. Please try again.");
    }
    setState((previous) => ({
      ...previous, enabled: { ...previous.enabled, ...(resetDraft ? { new: false } : {}), [threadId]: capturedEnabled },
      ...(resetDraft ? { warning: null, error: null } : {}),
    }));
  };
  const forgetThread = (threadId: string) => {
    if (accountId) forgetThreadWebAccess(browserConsentStorage(), accountId, threadId);
    setState((previous) => {
      if (previous.owner !== owner) return previous;
      const enabled = { ...previous.enabled };
      delete enabled[threadId];
      return { ...previous, enabled };
    });
  };
  const error = owner !== null && state.owner === owner ? state.error : null;
  return { enabled, warning, error, toggle, confirm, cancel, resetNew, adoptThread, forgetThread };
}

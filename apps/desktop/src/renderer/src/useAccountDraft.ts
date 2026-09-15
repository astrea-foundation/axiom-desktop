import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";

/** Hide a previous account's draft during render, before effects can run. */
export function useAccountDraft(accountId: string | null) {
  const [stored, setStored] = useState({ accountId, text: "" });
  const currentAccount = useRef(accountId);
  currentAccount.current = accountId;
  useEffect(() => {
    setStored((value) => value.accountId === accountId ? value : { accountId, text: value.accountId === null ? value.text : "" });
  }, [accountId]);
  const setDraft = useCallback((next: SetStateAction<string>) => {
    setStored((value) => {
      // An IPC completion from the old account cannot modify the new draft.
      if (currentAccount.current !== accountId) return value;
      const previous = value.accountId === accountId || value.accountId === null ? value.text : "";
      return { accountId, text: typeof next === "function" ? next(previous) : next };
    });
  }, [accountId]);
  // An anonymous draft can follow the user's first sign-in. Authored account
  // drafts are hidden immediately on sign-out or a different account.
  return [stored.accountId === accountId || stored.accountId === null ? stored.text : "", setDraft] as const;
}

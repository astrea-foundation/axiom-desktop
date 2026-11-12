import type { ClientState } from "@axiom/axiom-acp-client";
import { useCallback, useEffect, useRef, useState } from "react";
import { accountPresentation, signInErrorMessage, type SignInState } from "./signInFlow";

export function useNativeAccount(agentState: ClientState, desktopReady: boolean,
  refreshModels: () => void, clearModels: () => void,
  onOpenSignIn: () => void, setUiError: (error: string | null) => void) {
  const [signInState, setSignInState] = useState<SignInState>({ kind: "closed" });
  const signInGeneration = useRef(0);
  const signInPending = useRef(false);
  const activeLoginId = useRef<string | null>(null);
  const signInCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const accountView = accountPresentation(agentState.connected, desktopReady, agentState.account);
  const requiresSignIn = accountView.kind === "signed-out" || accountView.kind === "expired";
  const refreshAccount = useCallback(async () => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected) return;
    try {
      await api.accountStatus();
      refreshModels();
      setUiError(null);
    } catch (error) {
      setUiError(error instanceof Error ? error.message : String(error));
    }
  }, [agentState.connected, refreshModels, setUiError]);

  const logout = useCallback(() => {
    void (async () => {
      try {
        await window.axiomDesktop?.agent.logout();
        clearModels();
        setUiError(null);
      } catch (error) {
        setUiError(error instanceof Error ? error.message : String(error));
      }
    })();
  }, [clearModels, setUiError]);

  const refreshAfterAccountPortal = useRef(false);

  useEffect(() => {
    const refreshOnReturn = () => {
      if (!refreshAfterAccountPortal.current || !agentState.connected) return;
      refreshAfterAccountPortal.current = false;
      void refreshAccount();
    };
    window.addEventListener("focus", refreshOnReturn);
    return () => window.removeEventListener("focus", refreshOnReturn);
  }, [agentState.connected, refreshAccount]);

  const refreshBilling = useCallback(async () => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected || agentState.account?.state !== "valid") {
      throw new Error("Sign in to load billing");
    }
    await api.billingStatus();
  }, [agentState.account?.state, agentState.connected]);

  const cancelNativeSignIn = useCallback(() => {
    signInGeneration.current += 1;
    signInPending.current = false;
    if (signInCloseTimer.current) {
      clearTimeout(signInCloseTimer.current);
      signInCloseTimer.current = null;
    }
    const loginId = activeLoginId.current;
    activeLoginId.current = null;
    setSignInState({ kind: "closed" });
    if (loginId) {
      void window.axiomDesktop?.agent.nativeLoginCancel(loginId).catch(() => undefined);
    }
  }, []);

  const openSignIn = useCallback(() => {
    onOpenSignIn();
    setUiError(null);
    if (signInPending.current) return;
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected) {
      setSignInState({
        kind: "error",
        message: "AxiomCLI is not connected yet. Wait a moment and try again.",
      });
      return;
    }

    const generation = ++signInGeneration.current;
    signInPending.current = true;
    if (signInCloseTimer.current) {
      clearTimeout(signInCloseTimer.current);
      signInCloseTimer.current = null;
    }
    activeLoginId.current = null;
    setSignInState({ kind: "starting" });

    void (async () => {
      try {
        const started = await api.nativeLoginStart();
        const loginId = started.login.loginId;
        if (generation !== signInGeneration.current) {
          // A cancelled start can still finish its ACP round trip. Explicitly cancel
          // the short-lived server authorization without trying to complete it.
          void api.nativeLoginCancel(loginId).catch(() => undefined);
          return;
        }

        activeLoginId.current = loginId;
        setSignInState({ kind: "waiting", login: started.login });
        const completed = await api.nativeLoginComplete(loginId);
        if (activeLoginId.current === loginId) activeLoginId.current = null;
        if (generation !== signInGeneration.current) return;
        if (completed.status.state !== "valid") {
          throw new Error(completed.status.detail || "Axiom did not return a valid account after approval.");
        }

        signInPending.current = false;
        setSignInState({ kind: "success" });
        refreshModels();
        setUiError(null);
        signInCloseTimer.current = setTimeout(() => {
          if (generation === signInGeneration.current) setSignInState({ kind: "closed" });
          signInCloseTimer.current = null;
        }, 900);
      } catch (error) {
        if (generation !== signInGeneration.current) return;
        activeLoginId.current = null;
        signInPending.current = false;
        setSignInState({ kind: "error", message: signInErrorMessage(error) });
      }
    })();
  }, [agentState.connected, onOpenSignIn, refreshModels, setUiError]);

  useEffect(() => () => {
    signInGeneration.current += 1;
    if (signInCloseTimer.current) clearTimeout(signInCloseTimer.current);
    const loginId = activeLoginId.current;
    if (loginId) void window.axiomDesktop?.agent.nativeLoginCancel(loginId).catch(() => undefined);
  }, []);
  const openAccountPortal = () => {
    refreshAfterAccountPortal.current = true;
    void window.axiomDesktop?.agent.openAccountPortal().catch((error: unknown) => {
      refreshAfterAccountPortal.current = false;
      setUiError(error instanceof Error ? error.message : String(error));
    });
  };
  return { accountView, requiresSignIn, signInState, setSignInState, refreshAccount, logout, refreshBilling, cancelNativeSignIn, openSignIn, openAccountPortal };
}

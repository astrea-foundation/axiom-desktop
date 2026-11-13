import type { ClientState } from "@axiom/axiom-acp-client";
import { useEffect, useState } from "react";

const EMPTY_AGENT_STATE: ClientState = {
  connected: false,
  runtimeInstanceId: null,
  lastSequence: 0,
  sessions: {},
  catalog: [],
  collections: { revision: 0, collections: [] },
  preferences: null,
  account: null,
  billing: null,
  diagnostic: "",
  error: null,
};

export function useDesktopState(setUiError: (error: string | null) => void) {
  const [{ agentState, errorRevision }, setSnapshot] = useState({ agentState: EMPTY_AGENT_STATE, errorRevision: 0 });

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    if (!api) {
      setUiError("Axiom Desktop bridge is unavailable");
      return;
    }
    let disposed = false;
    let receivedLiveState = false;
    const setAgentState = (state: ClientState) => setSnapshot((previous) => ({
      agentState: state,
      // Count distinct error occurrences even when recovery and a new failure
      // arrive in one React batch. Ordinary state refreshes keep dismissal.
      errorRevision: previous.errorRevision + (state.error && state.error !== previous.agentState.error ? 1 : 0),
    }));
    const unsubscribe = api.onState((state) => {
      if (disposed) return;
      receivedLiveState = true;
      setAgentState(state);
    });
    void api.getState().then((state) => {
      // Subscription is installed first so an older IPC snapshot can never
      // overwrite a newer sidecar-ready event that raced its round trip.
      if (!disposed && !receivedLiveState) setAgentState(state);
    }).catch((error: unknown) => {
      if (!disposed) setUiError(String(error));
    });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, []);

  return { agentState, errorRevision };
}

import type { ClientState } from "@axiom/axiom-acp-client";
import { useEffect, useRef, useState } from "react";

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

const connectionScope = (state: ClientState) => JSON.stringify([
  state.connected, state.runtimeInstanceId,
  state.account?.state === "valid" ? state.account.account?.id ?? null : null,
]);

export function useDesktopState(setUiError: (error: string | null) => void) {
  const latestScope = useRef(connectionScope(EMPTY_AGENT_STATE));
  const latestConnectionRevision = useRef(0);
  const [{ agentState, errorRevision, connectionRevision }, setSnapshot] = useState({
    agentState: EMPTY_AGENT_STATE, errorRevision: 0, connectionRevision: 0,
  });

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    if (!api) {
      setUiError("Axiom Desktop bridge is unavailable");
      return;
    }
    let disposed = false;
    let receivedLiveState = false;
    const setAgentState = (state: ClientState) => {
      const scope = connectionScope(state);
      if (scope !== latestScope.current) {
        latestScope.current = scope;
        latestConnectionRevision.current++;
      }
      // Invalidate asynchronous work at notification ingestion, before React
      // renders. An away-and-back sequence can otherwise disappear in a batch.
      const revision = latestConnectionRevision.current;
      setSnapshot((previous) => ({
        agentState: state, connectionRevision: revision,
        // Count distinct error occurrences even when recovery and a new failure
        // arrive in one React batch. Ordinary state refreshes keep dismissal.
        errorRevision: previous.errorRevision + (state.error && state.error !== previous.agentState.error ? 1 : 0),
      }));
    };
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

  return { agentState, errorRevision, connectionContext: { revision: connectionRevision, latest: latestConnectionRevision } };
}

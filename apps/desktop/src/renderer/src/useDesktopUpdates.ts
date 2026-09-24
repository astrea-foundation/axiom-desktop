import {flushForUpdate, releaseUpdateFlush} from './updateFlush';
import { useCallback, useEffect, useState } from 'react';
import type { UpdateState, UpdatesApi } from '../../shared/updates';

export function useDesktopUpdates() {
  const [state, setState] = useState<UpdateState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const accept = useCallback((next: UpdateState) => {
    if (next.status !== 'installing') releaseUpdateFlush();
    setState(current => !current || next.revision >= current.revision ? next : current);
  }, []);
  useEffect(() => {
    const api = window.axiomDesktop?.updates;
    if (!api) return;
    let active = true;
    const beforeRestart = api.onBeforeRestart?.(flushForUpdate);
    const unsubscribe = api.onStateChange(next => { if (active) accept(next); });
    void api.getState().then(next => { if (active) accept(next); }).catch(() => {
      if (active) setError('Update controls are unavailable. Restart Axiom to try again.');
    });
    return () => { active = false; unsubscribe(); beforeRestart?.(); };
  }, [accept]);
  const run = useCallback(async (action: (api: UpdatesApi) => Promise<UpdateState | void>) => {
    const api = window.axiomDesktop?.updates;
    if (!api) return;
    setError(null);
    try { const next = await action(api); if (next) accept(next); }
    catch { setError('Couldn’t complete that update action. Please try again.'); }
  }, [accept]);
  return { state, error, check: () => run(api => api.check()),
    install: () => run(api => api.install()), cancel: () => run(api => api.cancel()) };
}
export type DesktopUpdates = ReturnType<typeof useDesktopUpdates>;

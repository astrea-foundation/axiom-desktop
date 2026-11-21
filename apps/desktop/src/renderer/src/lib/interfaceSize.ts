import { useEffect, useState } from "react";
import type { InterfaceSize, InterfaceSizeState } from "../../../shared/interface-size";

/**
 * The window's interface size, owned by the main process. Null while it loads
 * and wherever the host has no window zoom, so the control stays hidden.
 */
export function useInterfaceSize(): (InterfaceSizeState & { setSize: (size: InterfaceSize) => void }) | null {
  const api = typeof window === "undefined" ? undefined : window.axiomDesktop?.interfaceSize;
  const available = !!api;
  const [state, setState] = useState<InterfaceSizeState | null>(null);
  useEffect(() => {
    const api = window.axiomDesktop?.interfaceSize;
    if (!api) return;
    let active = true;
    void api.get().then((value) => { if (active) setState(value); }, () => {});
    // Keyboard changes are applied by the main process and only reported here.
    const stop = api.onChange((value) => { if (active) setState(value); });
    return () => { active = false; stop(); };
  }, [available]);
  if (!api || !state) return null;
  return { ...state, setSize: (size) => { void api.set(size).then(setState, () => {}); } };
}

import { ipcRenderer } from "electron";
import type { ProxyApi, ProxyState } from "../shared/proxy";

export const proxyApi: ProxyApi = {
  getState: () => ipcRenderer.invoke("proxy:state"),
  start: (port, accountId, runtimeId) => ipcRenderer.invoke("proxy:start", port, accountId, runtimeId),
  stop: () => ipcRenderer.invoke("proxy:stop"),
  // Only main writes the local token to the clipboard; no upstream credential
  // or local token is returned to the renderer or its state snapshots.
  copyToken: (accountId, runtimeId) => ipcRenderer.invoke("proxy:copy-token", accountId, runtimeId),
  onState: (callback) => {
    const listener = (_event: unknown, state: ProxyState) => callback(state);
    ipcRenderer.on("proxy:state-changed", listener);
    return () => ipcRenderer.removeListener("proxy:state-changed", listener);
  },
};

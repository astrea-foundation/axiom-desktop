import { contextBridge, ipcRenderer } from "electron";
import { agentApi, type AgentApi } from "./agent-api";

export type DesktopApi = {
  platform: NodeJS.Platform;
  minimize: () => void;
  maximize: () => void;
  close: () => void;
  isMaximized: () => Promise<boolean>;
  onMaximizedChange: (callback: (maximized: boolean) => void) => () => void;
  agent: AgentApi;
};

const api: DesktopApi = {
  platform: process.platform,
  minimize: () => {
    ipcRenderer.send("window:minimize");
  },
  maximize: () => {
    ipcRenderer.send("window:maximize");
  },
  close: () => {
    ipcRenderer.send("window:close");
  },
  isMaximized: () => ipcRenderer.invoke("window:isMaximized"),
  onMaximizedChange: (callback) => {
    const listener = (_event: unknown, maximized: boolean) => {
      callback(maximized);
    };
    ipcRenderer.on("window:maximized-changed", listener);
    return () => {
      ipcRenderer.removeListener("window:maximized-changed", listener);
    };
  },
  agent: agentApi,
};

contextBridge.exposeInMainWorld("axiomDesktop", api);

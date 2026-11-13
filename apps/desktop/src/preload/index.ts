import { contextBridge, ipcRenderer } from "electron";
import { agentApi, type AgentApi } from "./agent-api";
import { proxyApi } from "./proxy-api";
import type { ProxyApi } from "../shared/proxy";
import type { UpdatesApi } from "../shared/updates";

export type DesktopApi = {
  platform: NodeJS.Platform;
  minimize: () => void;
  maximize: () => void;
  close: () => void;
  isMaximized: () => Promise<boolean>;
  onMaximizedChange: (callback: (maximized: boolean) => void) => () => void;
  isFullScreen: () => Promise<boolean>;
  onFullScreenChange: (callback: (fullScreen: boolean) => void) => () => void;
  isFocused: () => Promise<boolean>;
  onActiveChange: (callback: (active: boolean) => void) => () => void;
  setTheme: (preference: "light" | "dark" | "system", resolved: "light" | "dark") => void;
  agent: AgentApi;
  proxy: ProxyApi;
  updates: UpdatesApi;
};

const previewArgument = process.argv.find((argument) => argument.startsWith("--axiom-platform-preview="));
const previewPlatform = previewArgument?.slice("--axiom-platform-preview=".length) as NodeJS.Platform | undefined;

const api: DesktopApi = {
  platform: previewPlatform ?? process.platform,
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
  isFullScreen: () => ipcRenderer.invoke("window:isFullScreen"),
  onFullScreenChange: (callback) => {
    const listener = (_event: unknown, fullScreen: boolean) => {
      callback(fullScreen);
    };
    ipcRenderer.on("window:fullscreen-changed", listener);
    return () => {
      ipcRenderer.removeListener("window:fullscreen-changed", listener);
    };
  },
  isFocused: () => ipcRenderer.invoke("window:isFocused"),
  setTheme: (preference, resolved) => {
    ipcRenderer.send("window:set-theme", preference, resolved);
  },
  onActiveChange: (callback) => {
    const listener = (_event: unknown, active: boolean) => {
      callback(active);
    };
    ipcRenderer.on("window:active-changed", listener);
    return () => {
      ipcRenderer.removeListener("window:active-changed", listener);
    };
  },
  agent: agentApi,
  proxy: proxyApi,
  updates: {
    getState: () => ipcRenderer.invoke("updates:state"),
    check: () => ipcRenderer.invoke("updates:check"),
    download: (format) => ipcRenderer.invoke("updates:download", format),
    reveal: () => ipcRenderer.invoke("updates:reveal"),
    onStateChange: (callback) => {
      const listener = (_event: unknown, state: Parameters<typeof callback>[0]) => callback(state);
      ipcRenderer.on("updates:state-changed", listener);
      return () => ipcRenderer.removeListener("updates:state-changed", listener);
    },
  },
};

contextBridge.exposeInMainWorld("axiomDesktop", api);

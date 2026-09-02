import { contextBridge, ipcRenderer } from "electron";
import { agentApi, type AgentApi } from "./agent-api";

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
};

contextBridge.exposeInMainWorld("axiomDesktop", api);

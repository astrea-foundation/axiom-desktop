import { contextBridge, ipcRenderer } from "electron";
import { agentApi, type AgentApi } from "./agent-api";

export type DesktopApi = {
  platform: NodeJS.Platform;
  isFullScreen: () => Promise<boolean>;
  onFullScreenChange: (callback: (fullScreen: boolean) => void) => () => void;
  isFocused: () => Promise<boolean>;
  onActiveChange: (callback: (active: boolean) => void) => () => void;
  setTheme: (preference: "light" | "dark" | "system", resolved: "light" | "dark") => void;
  agent: AgentApi;
};

const previewArgument = process.argv.find((argument) => argument.startsWith("--axiom-platform-preview="));
const previewPlatform = previewArgument?.slice("--axiom-platform-preview=".length) as NodeJS.Platform | undefined;

const api: DesktopApi = {
  platform: previewPlatform ?? process.platform,
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
};

contextBridge.exposeInMainWorld("axiomDesktop", api);

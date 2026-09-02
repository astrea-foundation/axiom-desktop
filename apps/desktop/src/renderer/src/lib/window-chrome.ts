import { useEffect, useState } from "react";

const desktop = typeof window === "undefined" ? undefined : window.axiomDesktop;
const platform = desktop?.platform;

export const isMac = platform === "darwin";
export const isWindows = platform === "win32";

/** macOS traffic lights and the Windows title-bar overlay are drawn by the OS. */
export const hasNativeWindowControls = isMac || isWindows;

/** Width reserved for the macOS traffic lights when they overlap our chrome. */
export const MAC_TRAFFIC_LIGHT_INSET_CLASS = "pl-[76px]";

/** Right inset for the native Windows caption buttons, sized from the WCO env vars. */
export const WINDOWS_CAPTION_INSET_CLASS = "pad-titlebar-overlay";

// Fullscreen state is cached at module level so components that mount later
// (e.g. the sidebar after reopening) paint the correct inset on first render.
let cachedFullScreen = false;
let subscribed = false;
const fullScreenListeners = new Set<(fullScreen: boolean) => void>();

function broadcastFullScreen(fullScreen: boolean): void {
  cachedFullScreen = fullScreen;
  for (const listener of fullScreenListeners) listener(fullScreen);
}

function ensureFullScreenSubscription(): void {
  if (subscribed || !desktop) return;
  subscribed = true;
  void desktop.isFullScreen().then(broadcastFullScreen);
  desktop.onFullScreenChange(broadcastFullScreen);
}

export function useFullScreen(): boolean {
  const [fullScreen, setFullScreen] = useState(cachedFullScreen);

  useEffect(() => {
    ensureFullScreenSubscription();
    setFullScreen(cachedFullScreen);
    fullScreenListeners.add(setFullScreen);
    return () => {
      fullScreenListeners.delete(setFullScreen);
    };
  }, []);

  return fullScreen;
}

let cachedActive = true;
let activeSubscribed = false;
const activeListeners = new Set<(active: boolean) => void>();

function broadcastActive(active: boolean): void {
  cachedActive = active;
  for (const listener of activeListeners) listener(active);
}

function ensureActiveSubscription(): void {
  if (activeSubscribed || !desktop) return;
  activeSubscribed = true;
  void desktop.isFocused().then(broadcastActive);
  desktop.onActiveChange(broadcastActive);
}

export function useWindowActive(): boolean {
  const [active, setActive] = useState(cachedActive);

  useEffect(() => {
    ensureActiveSubscription();
    setActive(cachedActive);
    activeListeners.add(setActive);
    return () => {
      activeListeners.delete(setActive);
    };
  }, []);

  return active;
}

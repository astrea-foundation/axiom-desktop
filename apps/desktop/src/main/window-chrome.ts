import type { BrowserWindowConstructorOptions, TitleBarOverlay } from "electron";

export type WindowChromeTheme = "light" | "dark";

export const WINDOW_CONTROLS_OVERLAY_HEIGHT = 44;

export function windowControlsOverlay(theme: WindowChromeTheme): TitleBarOverlay {
  return {
    color: "#00000000",
    symbolColor: theme === "dark" ? "#f4f4f6" : "#101014",
    height: WINDOW_CONTROLS_OVERLAY_HEIGHT,
  };
}

/**
 * Per-platform window chrome. macOS keeps native traffic lights over a hidden
 * title bar; Windows and Linux use Electron's native window-controls overlay.
 */
export function platformWindowChrome(
  platform: NodeJS.Platform,
  theme: WindowChromeTheme = "light",
): BrowserWindowConstructorOptions {
  if (platform === "darwin") {
    // hiddenInset hosts the traffic lights in a native toolbar, which keeps
    // their inactive (grey) rendering when the window loses focus. A custom
    // trafficLightPosition re-parents them into a container that skips that
    // pass, so the buttons draw empty on blur (electron#27295).
    return { titleBarStyle: "hiddenInset" };
  }
  return {
    titleBarStyle: "hidden",
    titleBarOverlay: windowControlsOverlay(theme),
  };
}

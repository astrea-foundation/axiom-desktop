import type { BrowserWindowConstructorOptions } from "electron";

/**
 * Per-platform window chrome. macOS keeps native traffic lights over a hidden
 * title bar; Windows and Linux are frameless with controls drawn in the
 * canvas cutout by the renderer.
 */
export function platformWindowChrome(platform: NodeJS.Platform): BrowserWindowConstructorOptions {
  if (platform === "darwin") {
    // hiddenInset hosts the traffic lights in a native toolbar, which keeps
    // their inactive (grey) rendering when the window loses focus. A custom
    // trafficLightPosition re-parents them into a container that skips that
    // pass, so the buttons draw empty on blur (electron#27295).
    return { titleBarStyle: "hiddenInset" };
  }
  return { frame: false };
}

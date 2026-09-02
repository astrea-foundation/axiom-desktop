import type { BrowserWindowConstructorOptions } from "electron";

/**
 * Per-platform window chrome. macOS keeps native traffic lights over a hidden
 * title bar; Windows uses the native controls overlay (snap layouts included);
 * Linux stays frameless with DOM-drawn controls.
 */
export function platformWindowChrome(platform: NodeJS.Platform): BrowserWindowConstructorOptions {
  if (platform === "darwin") {
    // hiddenInset hosts the traffic lights in a native toolbar, which keeps
    // their inactive (grey) rendering when the window loses focus. A custom
    // trafficLightPosition re-parents them into a container that skips that
    // pass, so the buttons draw empty on blur (electron#27295).
    return { titleBarStyle: "hiddenInset" };
  }
  if (platform === "win32") {
    // The overlay mostly covers the floating stage card's top-right corner
    // (white at 72% over the canvas), not the raw canvas background.
    return {
      titleBarStyle: "hidden",
      titleBarOverlay: { color: "#fbfbfc", symbolColor: "#56565f", height: 44 },
    };
  }
  return { frame: false };
}

import assert from "node:assert/strict";
import { test } from "node:test";
import { platformWindowChrome } from "../src/main/window-chrome";
import {
  hasNativeWindowControls,
  isLinux,
  isMac,
  isWindows,
  MAC_TRAFFIC_LIGHT_INSET_CLASS,
} from "../src/renderer/src/lib/window-chrome";

test("macOS chrome keeps native traffic lights over a hidden title bar", () => {
  const chrome = platformWindowChrome("darwin");
  assert.equal(chrome.titleBarStyle, "hiddenInset");
  // Native toolbar placement on purpose: a custom trafficLightPosition makes
  // the buttons draw empty when the window is inactive (electron#27295).
  assert.equal(chrome.trafficLightPosition, undefined);
  assert.equal(chrome.frame, undefined);
  assert.equal(chrome.titleBarOverlay, undefined);
});

test("Windows chrome is frameless with controls in the canvas cutout", () => {
  assert.deepEqual(platformWindowChrome("win32"), { frame: false });
});

test("Linux chrome is frameless with controls in the canvas cutout", () => {
  assert.deepEqual(platformWindowChrome("linux"), { frame: false });
});

test("renderer window-chrome module is inert outside a browser window", () => {
  assert.equal(isMac, false);
  assert.equal(isWindows, false);
  assert.equal(isLinux, false);
  assert.equal(hasNativeWindowControls, false);
  assert.ok(MAC_TRAFFIC_LIGHT_INSET_CLASS.startsWith("pl-"));
});

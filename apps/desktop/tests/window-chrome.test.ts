import assert from "node:assert/strict";
import { test } from "node:test";
import { platformWindowChrome } from "../src/main/window-chrome";
import {
  hasNativeWindowControls,
  isMac,
  isWindows,
  MAC_TRAFFIC_LIGHT_INSET_CLASS,
  WINDOWS_CAPTION_INSET_CLASS,
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

test("Windows chrome uses the native controls overlay matched to the window background", () => {
  const chrome = platformWindowChrome("win32");
  assert.equal(chrome.titleBarStyle, "hidden");
  assert.deepEqual(chrome.titleBarOverlay, {
    color: "#fbfbfc",
    symbolColor: "#56565f",
    height: 44,
  });
  assert.equal(chrome.frame, undefined);
});

test("Linux chrome stays frameless with DOM-drawn controls", () => {
  const chrome = platformWindowChrome("linux");
  assert.deepEqual(chrome, { frame: false });
});

test("renderer window-chrome module is inert outside a browser window", () => {
  assert.equal(isMac, false);
  assert.equal(isWindows, false);
  assert.equal(hasNativeWindowControls, false);
  assert.ok(MAC_TRAFFIC_LIGHT_INSET_CLASS.startsWith("pl-"));
  assert.equal(WINDOWS_CAPTION_INSET_CLASS, "pad-titlebar-overlay");
});

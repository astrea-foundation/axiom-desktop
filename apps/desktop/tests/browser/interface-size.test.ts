import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The real Settings sheet in a browser. The main process owns the window zoom,
// so the bridge here only records what the control asks for and reports back.
let browser: Browser, server: ViteDevServer, origin: string;
before(async () => {
  server = await createServer({ cacheDir: fileURLToPath(new URL("../../node_modules/.vite-tests/interface-size.test", import.meta.url)), configFile: false,
    root: fileURLToPath(new URL("../../src/renderer", import.meta.url)), plugins: [react(), tailwindcss()], server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
  await server.listen(); origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
});
after(async () => { await browser?.close(); await server?.close(); });

async function open(bridge: { platform: string; size: number; defaultSize: number } | null, width = 1120) {
  const page = await browser.newPage({ viewport: { width, height: 840 }, bypassCSP: true });
  const start = new URL(origin); start.searchParams.set("state", "signed_out"); start.searchParams.set("category", "appearance");
  page.setDefaultTimeout(8000);
  const errors: string[] = []; page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript({ content: "window.__name = (fn) => fn;" });
  await page.addInitScript((bridge) => {
    const requested: number[] = [];
    let state = bridge ? { size: bridge.size, defaultSize: bridge.defaultSize } : null;
    let notify: ((value: unknown) => void) | null = null;
    Object.assign(window, {
      __interfaceSize: { requested, shortcut: (size: number) => { state = { ...state!, size }; notify?.(state); }, listening: () => notify !== null },
      axiomDesktop: { platform: bridge?.platform ?? "linux", setTheme: () => {}, ...(bridge ? { interfaceSize: {
        get: async () => state,
        set: async (size: number) => { requested.push(size); state = { ...state!, size }; return state; },
        onChange: (callback: (value: unknown) => void) => { notify = callback; return () => { notify = null; }; },
      } } : {}) },
    });
  }, bridge);
  await page.route("**/*", (route) => {
    const url = route.request().url();
    if (url === start.href) return route.fulfill({ contentType: "text/html", body: `<!doctype html><html data-theme="dark"><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module">import RefreshRuntime from "/@react-refresh"; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type; window.__vite_plugin_react_preamble_installed__ = true; await import("/@fs/${fileURLToPath(new URL("./fixtures/factory-reset.tsx", import.meta.url))}");</script></body></html>` });
    return url.startsWith(origin) ? route.continue() : route.abort();
  });
  await page.goto(start.href);
  await page.getByRole("radiogroup", { name: "Theme" }).waitFor();
  return { page, errors };
}
const requested = (page: import("playwright").Page) => page.evaluate(() => (window as any).__interfaceSize.requested);

test("Appearance offers the four sizes, marks the platform default, and asks the main process to change", async () => {
  const { page, errors } = await open({ platform: "win32", size: 110, defaultSize: 110 });
  try {
    const group = page.getByRole("radiogroup", { name: "Interface size" });
    await group.waitFor();
    assert.deepEqual(await group.getByRole("radio").allInnerTexts(), ["100%", "110%", "125%", "150%"]);
    assert.equal(await group.getByRole("radio", { name: "110%, default", exact: true }).getAttribute("aria-checked"), "true");
    assert.match(await page.getByRole("tabpanel").innerText(), /Also Ctrl \+ and Ctrl −\./);
    await group.getByRole("radio", { name: "125%", exact: true }).click();
    assert.deepEqual(await requested(page), [125]);
    assert.equal(await group.getByRole("radio", { name: "125%", exact: true }).getAttribute("aria-checked"), "true");
    assert.equal(await group.getByRole("radio", { name: "110%, default", exact: true }).getAttribute("aria-checked"), "false");
    // Arrow keys move the choice and the focus together, like the theme control.
    await page.keyboard.press("ArrowRight");
    assert.deepEqual(await requested(page), [125, 150]);
    assert.equal(await page.evaluate(() => document.activeElement?.textContent), "150%");
    await page.keyboard.press("Home");
    assert.deepEqual(await requested(page), [125, 150, 100]);
    assert.equal(await group.getByRole("radio", { checked: true }).innerText(), "100%");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("a keyboard change made by the main process shows in the control without a request from it", async () => {
  const { page, errors } = await open({ platform: "darwin", size: 100, defaultSize: 100 });
  try {
    const group = page.getByRole("radiogroup", { name: "Interface size" });
    await group.waitFor();
    assert.match(await page.getByRole("tabpanel").innerText(), /Also ⌘ \+ and ⌘ −\./);
    assert.equal(await group.getByRole("radio", { name: "100%, default", exact: true }).getAttribute("aria-checked"), "true");
    await page.waitForFunction(() => (window as any).__interfaceSize.listening());
    await page.evaluate(() => (window as any).__interfaceSize.shortcut(125));
    await page.waitForFunction(() => document.querySelector('[role="radiogroup"][aria-label="Interface size"] [aria-checked="true"]')?.textContent === "125%");
    assert.deepEqual(await requested(page), []);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("the control is absent where the host has no window zoom, and wraps in a narrow panel", async () => {
  const without = await open(null);
  try {
    assert.equal(await without.page.getByRole("radiogroup", { name: "Interface size" }).count(), 0);
    assert.equal(await without.page.getByText("Interface size").count(), 0);
    assert.deepEqual(without.errors, []);
  } finally { await without.page.close(); }
  const narrow = await open({ platform: "win32", size: 150, defaultSize: 110 }, 720);
  try {
    const group = narrow.page.getByRole("radiogroup", { name: "Interface size" });
    await group.waitFor();
    const box = (await group.boundingBox())!;
    assert.ok(box.x >= 0 && box.x + box.width <= 720, "the size control stays inside a 720px layout");
    assert.equal(await narrow.page.evaluate(() => document.scrollingElement!.scrollWidth <= innerWidth), true);
    assert.deepEqual(narrow.errors, []);
  } finally { await narrow.page.close(); }
});

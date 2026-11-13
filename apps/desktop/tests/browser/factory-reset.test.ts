import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

let browser: Browser, server: ViteDevServer, origin: string;
const captures = process.env.AXIOM_RESET_CAPTURE_DIR;
before(async () => {
  server = await createServer({ cacheDir: fileURLToPath(new URL("../../node_modules/.vite-tests/factory-reset.test", import.meta.url)), configFile: false, root: fileURLToPath(new URL("../../src/renderer", import.meta.url)),
    plugins: [react(), tailwindcss()], server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
  await server.listen(); origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
  if (captures) await mkdir(captures, { recursive: true });
});
after(async () => { await browser?.close(); await server?.close(); });

async function open(theme: string, state = "disconnected", width = 1120, long = false) {
  const page = await browser.newPage({ viewport: { width, height: 840 }, bypassCSP: true });
  const start = new URL(origin); start.searchParams.set("state", state);
  if (long) start.searchParams.set("long", "1");
  page.setDefaultTimeout(8000);
  const errors: string[] = []; page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript({ content: "window.__name = (fn) => fn;" });
  await page.addInitScript((theme) => {
    localStorage.setItem("axiom.theme", theme);
    let calls = 0, failed = false;
    let finish: (() => void) | null = null;
    Object.assign(window, {
      __resetTest: { calls: () => calls, fail: () => { failed = true; }, finish: () => finish?.() },
      axiomDesktop: { setTheme: () => {}, factoryReset: async (confirmation: string) => {
        if (confirmation !== "RESET") throw new Error("Invalid confirmation");
        calls++;
        if (failed) throw new Error("Native reset helper is unavailable. Update Axiom and retry.");
        return new Promise<void>((resolve) => { finish = resolve; });
      } },
    });
  }, theme);
  await page.route("**/*", (route) => {
    const url = route.request().url();
    if (url === start.href) return route.fulfill({ contentType: "text/html", body: `<!doctype html><html data-theme="${theme}"><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module">import RefreshRuntime from "/@react-refresh"; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type; window.__vite_plugin_react_preamble_installed__ = true; await import("/@fs/${fileURLToPath(new URL("./fixtures/factory-reset.tsx", import.meta.url))}");</script></body></html>` });
    return url.startsWith(origin) ? route.continue() : route.abort();
  });
  await page.goto(start.href);
  await page.getByRole("button", { name: "Factory reset Axiom…", exact: true }).waitFor();
  await page.evaluate(() => document.fonts.ready);
  return { page, errors };
}

for (const theme of ["light", "dark"]) test(`${theme}: signed-out reset requires exact typing, supports cancel, and submits only once`, async () => {
  const { page, errors } = await open(theme);
  try {
    const trigger = page.getByRole("button", { name: "Factory reset Axiom…", exact: true });
    assert.equal(await trigger.isEnabled(), true);
    if (captures) await page.screenshot({ path: `${captures}/${theme}-settings.png` });
    await trigger.click();
    const modal = page.getByRole("alertdialog", { name: "Factory reset Axiom?", exact: true });
    const submit = modal.getByRole("button", { name: "Delete everything and restart", exact: true });
    const confirmation = modal.getByLabel("Type RESET to confirm");
    assert.match(await modal.innerText(), /every account/);
    assert.match(await modal.innerText(), /system credential store/);
    assert.match(await modal.innerText(), /online account, balance, API keys/);
    assert.equal(await submit.isDisabled(), true);
    await confirmation.fill("reset");
    assert.equal(await submit.isDisabled(), true);
    await confirmation.fill("RESET");
    assert.equal(await submit.isEnabled(), true);
    await page.keyboard.press("Escape");
    assert.equal(await trigger.evaluate((button) => button === document.activeElement), true);
    assert.equal(await page.evaluate(() => (window as any).__resetTest.calls()), 0);
    await trigger.click();
    assert.equal(await confirmation.inputValue(), "");
    await confirmation.fill("RESET");
    if (captures) await page.screenshot({ path: `${captures}/${theme}-confirmation.png` });
    await submit.click();
    await modal.getByRole("status").waitFor();
    assert.equal(await modal.getByRole("button", { name: "Cancel", exact: true }).isDisabled(), true);
    assert.equal(await modal.getByRole("button", { name: "Restarting…", exact: true }).isDisabled(), true);
    await page.keyboard.press("Escape");
    assert.equal(await modal.isVisible(), true);
    assert.equal(await page.evaluate(() => (window as any).__resetTest.calls()), 1);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

for (const theme of ["light", "dark"]) test(`${theme}: account identity stays together and actions remain keyboard accessible`, async () => {
  const { page, errors } = await open(theme, "valid", 1320);
  try {
    const panel = page.getByRole("tabpanel", {name: "Account", exact: true});
    const avatar = await panel.locator("span.rounded-full").first().boundingBox();
    const name = await panel.getByText("Alex Morgan", {exact: true}).boundingBox();
    assert(avatar && name);
    assert(name.x - avatar.x - avatar.width >= 0 && name.x - avatar.x - avatar.width <= 24, "name must sit beside the avatar");
    assert(Math.abs(name.y - avatar.y) <= 12, "identity is aligned at the top of its card");
    await panel.getByText("alex@example.com", {exact: true}).waitFor();
    await panel.getByText("Google, Passkey", {exact: true}).waitFor();
    if (captures) await page.screenshot({path: `${captures}/${theme}-account-1320.png`});
    await panel.getByRole("button", {name: "Refresh account", exact: true}).click();
    await panel.getByRole("button", {name: "Manage account", exact: true}).click();
    await panel.getByRole("button", {name: "Sign out", exact: true}).click();
    assert.deepEqual(await page.evaluate(() => (window as any).__accountActions), ["refresh", "manage", "logout"]);
    const tab = page.getByRole("tab", {name: "Account", exact: true});
    await tab.focus(); await page.keyboard.press("ArrowUp");
    await page.getByRole("tab", {name: "Appearance", selected: true}).waitFor();
    await page.keyboard.press("ArrowDown");
    assert.equal(await tab.getAttribute("aria-selected"), "true");
    const focus = await tab.evaluate(element => ({focused: element === document.activeElement,
      visible: element.matches(":focus-visible"), offset: parseFloat(getComputedStyle(element).outlineOffset),
      width: parseFloat(getComputedStyle(element).outlineWidth)}));
    assert(focus.focused && focus.visible && focus.width > 0 && focus.offset < 0, "keyboard focus stays visible inside the scrolling tablist");
    assert.equal(await page.evaluate(() => (window as any).__resetTest.calls()), 0);
    await page.setViewportSize({width: 960, height: 640});
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    if (captures) await page.screenshot({path: `${captures}/${theme}-account-960.png`});
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

for (const state of ["disconnected", "signed_out", "expired", "starting"]) test(`account ${state} retains its sign-in action and reset access`, async () => {
  const { page, errors } = await open("dark", state, 960);
  try {
    const panel = page.getByRole("tabpanel", {name: "Account", exact: true});
    const waiting = state === "disconnected" || state === "starting";
    const signIn = panel.getByRole("button", {name: waiting ? "Getting ready…" : state === "expired" ? "Sign in again" : "Sign in", exact: true});
    assert.equal(await signIn.isDisabled(), waiting);
    if (!waiting) {
      await signIn.click();
      assert.deepEqual(await page.evaluate(() => (window as any).__accountActions), ["login"]);
    }
    assert.equal(await panel.getByRole("button", {name: "Factory reset Axiom…", exact: true}).isEnabled(), true);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("long account identity wraps within a narrow settings panel", async () => {
  const { page, errors } = await open("light", "valid", 960, true);
  try {
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    const panel = page.getByRole("tabpanel", {name: "Account", exact: true});
    assert.equal(await panel.evaluate(element => element.scrollWidth <= element.clientWidth), true);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("reset launch failures remain visible and allow cancellation and retry", async () => {
  const { page, errors } = await open("light");
  try {
    await page.evaluate(() => (window as any).__resetTest.fail());
    await page.getByRole("button", { name: "Factory reset Axiom…", exact: true }).click();
    const modal = page.getByRole("alertdialog", { name: "Factory reset Axiom?", exact: true });
    await modal.getByLabel("Type RESET to confirm").fill("RESET");
    await modal.getByRole("button", { name: "Delete everything and restart", exact: true }).click();
    await modal.getByRole("alert").waitFor();
    assert.match(await modal.getByRole("alert").innerText(), /helper is unavailable/);
    assert.equal(await modal.getByRole("button", { name: "Delete everything and restart", exact: true }).isEnabled(), true);
    await modal.getByRole("button", { name: "Cancel", exact: true }).click();
    assert.equal(await modal.isVisible(), false);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

let browser: Browser, server: ViteDevServer, origin: string;
before(async () => {
  server = await createServer({ configFile: false, root: fileURLToPath(new URL("../../src/renderer", import.meta.url)),
    plugins: [react(), tailwindcss()], server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
  await server.listen();
  origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
  // Let Vite finish its first dependency discovery/reload before installing
  // fake clocks; a cold optimizer reload otherwise races the first test page.
  const warmup = await browser.newPage();
  await warmup.route("**/*", (route) => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await warmup.goto(origin);
  await warmup.locator("#root > *").waitFor();
  await warmup.close();
});
after(async () => { await browser?.close(); await server?.close(); });

async function open(reducedMotion: "reduce" | "no-preference" = "no-preference") {
  const context = await browser.newContext({ viewport: { width: 1320, height: 860 }, reducedMotion });
  let dismissed = false;
  await context.exposeFunction("bannerIsDismissed", () => dismissed);
  await context.exposeFunction("dismissBanner", () => { dismissed = true; });
  const page = await context.newPage();
  await page.addInitScript({ content: "window.__name = (fn) => fn;" });
  await page.addInitScript(() => {
    const host = window as unknown as { bannerIsDismissed: () => Promise<boolean>; dismissBanner: () => Promise<void> };
    Object.assign(window, { axiomDesktop: {
      platform: "darwin", setTheme: () => {},
      isFullScreen: async () => false, onFullScreenChange: () => () => {},
      isFocused: async () => true, onActiveChange: () => () => {},
      astreaBanner: { isDismissed: () => host.bannerIsDismissed(), dismiss: () => host.dismissBanner() },
      agent: {
        getState: async () => ({ connected: false, runtimeInstanceId: null, sessions: {}, activeSessionId: null,
          catalog: [], collections: { revision: 0, collections: [] }, preferences: null, account: null, billing: null, diagnostic: "", error: null }),
        onState: () => () => {},
      },
    } });
  });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/*", (route) => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await page.clock.install({ time: new Date("2026-01-01T00:00:00Z") });
  await page.clock.pauseAt(new Date("2026-01-01T00:00:00.001Z"));
  await page.goto(origin);
  try { await page.locator(".astrea-banner").waitFor({ timeout: 8_000 }); }
  catch (error) { console.error(errors, await page.locator("body").innerText()); await context.close(); throw error; }
  await page.clock.runFor(400);
  await page.locator(".astrea-banner").evaluate((node) => node.getAnimations().forEach((animation) => animation.finish()));
  return { page, context, errors };
}

test("compact sidebar banner stays above the account with one-line branding and looping motion", async () => {
  const { page, context, errors } = await open();
  try {
    const banner = page.locator(".astrea-banner");
    const link = banner.getByRole("link", { name: /Astrea/ });
    assert.equal(await link.getAttribute("href"), "https://astrea.systems");
    assert.equal(await link.getAttribute("target"), "_blank");
    await banner.getByRole("img", { name: "Astrea", exact: true }).waitFor();
    await page.waitForFunction(() => document.querySelector("video")?.readyState === 4);
    const bounds = (await banner.boundingBox())!;
    const account = (await page.getByRole("button", { name: "Account menu", exact: true }).boundingBox())!;
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width < 270);
    assert.ok(bounds.height <= 56 && bounds.width > bounds.height * 3);
    assert.ok(bounds.y + bounds.height <= account.y);
    const copy = (await banner.locator(".astrea-banner__copy").boundingBox())!;
    const logo = (await banner.locator(".astrea-banner__lockup").boundingBox())!;
    assert.ok(Math.abs(copy.y + copy.height / 2 - logo.y - logo.height / 2) < 2);
    await page.clock.runFor(60_000);
    assert.equal(await banner.getAttribute("data-closing"), "false");
    assert.equal(await banner.getAttribute("inert"), null);
    assert.equal(await banner.locator("video").count(), 1);
    assert.equal(await banner.locator("video").evaluate((video) => video.loop && !video.paused), true);
    assert.equal(await banner.locator(".astrea-banner__motion").evaluate((node) => getComputedStyle(node).animationName), "none");
    assert.equal(await banner.isVisible(), true);
    assert.deepEqual(errors, []);
  } finally { await context.close(); }
});

test("close removes the sidebar banner and survives reload until a new desktop session", async () => {
  const { page, context, errors } = await open();
  try {
    await page.getByRole("button", { name: "Close Astrea banner until next launch" }).click();
    await page.clock.runFor(200);
    assert.equal(await page.locator(".astrea-banner").count(), 0);
    await page.clock.runFor(20_000);
    assert.equal(await page.locator(".astrea-banner").count(), 0);
    await page.reload();
    await page.getByRole("button", { name: "New thread", exact: true }).waitFor();
    await page.clock.runFor(10_000);
    assert.equal(await page.locator(".astrea-banner").count(), 0);
    assert.deepEqual(errors, []);
  } finally { await context.close(); }
  const nextLaunch = await open();
  try { assert.equal(await nextLaunch.page.locator(".astrea-banner").count(), 1); }
  finally { await nextLaunch.context.close(); }
});

test("reduced motion keeps the wordmark and close control without playing video", async () => {
  const { page, context, errors } = await open("reduce");
  try {
    assert.equal(await page.locator(".astrea-banner video").count(), 0);
    assert.equal(await page.locator(".astrea-banner").evaluate((node) => getComputedStyle(node).transitionDuration), "0s");
    await page.getByRole("button", { name: "Close Astrea banner until next launch" }).click();
    await page.clock.runFor(1);
    assert.equal(await page.locator(".astrea-banner").count(), 0);
    assert.deepEqual(errors, []);
  } finally { await context.close(); }
});

import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { claimAstreaBanner } from "../../src/main/astrea-banner";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

let browser: Browser, server: ViteDevServer, origin: string;
const profiles: string[] = [];
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
after(async () => { await browser?.close(); await server?.close(); await Promise.all(profiles.map(profile => rm(profile, { recursive: true, force: true }))); });

async function open(reducedMotion: "reduce" | "no-preference" = "no-preference", existingProfile?: string, visible = true) {
  const context = await browser.newContext({ viewport: { width: 1320, height: 860 }, reducedMotion });
  const profile = existingProfile ?? await mkdtemp(join(tmpdir(), "axiom-astrea-browser-"));
  if (!existingProfile) profiles.push(profile);
  await context.exposeFunction("claimBanner", () => claimAstreaBanner(profile));
  const page = await context.newPage();
  await page.addInitScript({ content: "window.__name = (fn) => fn;" });
  await page.addInitScript(() => {
    const host = window as unknown as { claimBanner: () => Promise<boolean> };
    Object.assign(window, { axiomDesktop: {
      platform: "darwin", setTheme: () => {},
      isFullScreen: async () => false, onFullScreenChange: () => () => {},
      isFocused: async () => true, onActiveChange: () => () => {},
      astreaBanner: { claim: () => host.claimBanner() },
      agent: {
        getState: async () => ({ connected: true, runtimeInstanceId: "banner-fixture", sessions: {}, activeSessionId: null,
          catalog: [], collections: { revision: 0, collections: [] }, preferences: null,
          account: { revision: 1, state: "valid", account: { id: "banner-fixture", displayName: "Preview account", linkedMethods: [] } }, billing: null, diagnostic: "", error: null }),
        onState: () => () => {},
        desktopBootstrap: async () => ({ newThreadSettings: { model: "auto", thinkingLevel: "medium" } }),
        listModels: async () => ({ models: [] }),
        localPayload: async () => ({}),
      },
    } });
  });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/*", (route) => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await page.clock.install({ time: new Date("2026-01-01T00:00:00Z") });
  await page.clock.pauseAt(new Date("2026-01-01T00:00:00.001Z"));
  await page.goto(origin);
  try {
    if (visible) await page.locator(".astrea-banner").waitFor({ timeout: 8_000 });
    else await page.getByRole("button", { name: "Account menu", exact: true }).filter({ hasText: "Preview account" }).waitFor({ timeout: 8_000 });
  }
  catch (error) { console.error(errors, await page.locator("body").innerText()); await context.close(); throw error; }
  await page.clock.runFor(400);
  if (visible) await page.locator(".astrea-banner").evaluate((node) => node.getAnimations().forEach((animation) => animation.finish()));
  return { page, context, errors, profile };
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

test("dismissal survives renderer reload and a new desktop session", async () => {
  const { page, context, errors, profile } = await open();
  try {
    await page.getByRole("button", { name: "Dismiss Astrea message" }).click();
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
  const nextLaunch = await open("no-preference", profile, false);
  try { assert.equal(await nextLaunch.page.locator(".astrea-banner").count(), 0); }
  finally { await nextLaunch.context.close(); }
});

test("an introduction left open is still shown in only its first session", async () => {
  const first = await open();
  await first.context.close();
  const second = await open("no-preference", first.profile, false);
  try { assert.equal(await second.page.locator(".astrea-banner").count(), 0); }
  finally { await second.context.close(); }
});

test("reduced motion keeps the wordmark and close control without playing video", async () => {
  const { page, context, errors } = await open("reduce");
  try {
    assert.equal(await page.locator(".astrea-banner video").count(), 0);
    assert.equal(await page.locator(".astrea-banner").evaluate((node) => getComputedStyle(node).transitionDuration), "0s");
    await page.getByRole("button", { name: "Dismiss Astrea message" }).click();
    await page.clock.runFor(1);
    assert.equal(await page.locator(".astrea-banner").count(), 0);
    assert.deepEqual(errors, []);
  } finally { await context.close(); }
});

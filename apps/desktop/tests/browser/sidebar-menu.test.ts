import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

let browser: Browser, server: ViteDevServer, origin: string;
before(async () => {
  server = await createServer({ cacheDir: fileURLToPath(new URL("../../node_modules/.vite-tests/sidebar-menu.test", import.meta.url)), configFile: false,
    root: fileURLToPath(new URL("../../", import.meta.url)), plugins: [react(), tailwindcss()], server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
  await server.listen(); origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
});
after(async () => { await browser?.close(); await server?.close(); });

test("the thread menu stays open while a reply scrolls the transcript and closes when its own list scrolls", async () => {
  const page = await browser.newPage({ viewport: { width: 900, height: 400 } });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  try {
    await page.goto(`${origin}tests/browser/sidebar-menu.fixture.html`);
    const trigger = page.getByRole("button", { name: "Actions for Test thread", exact: true });
    const menu = page.getByRole("menu", { name: "Thread actions", exact: true });
    await trigger.click();
    await menu.waitFor();
    // Streaming output keeps the transcript pinned to the bottom, one scroll per chunk.
    for (const top of [400, 1200, 3000]) {
      await page.locator("[data-transcript]").evaluate((element, value) => { element.scrollTop = value; }, top);
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    }
    assert.equal(await page.locator("[data-transcript]").evaluate((element) => element.scrollTop), 3000);
    assert.equal(await menu.isVisible(), true);
    await menu.getByRole("menuitem", { name: "Rename", exact: true }).click();
    assert.equal(await page.locator("output").innerText(), "rename");

    // The menu is anchored to its row, so scrolling that list still dismisses it.
    await trigger.click();
    await menu.waitFor();
    await page.locator("[data-thread-list]").evaluate((element) => { element.scrollTop = 200; });
    await menu.waitFor({ state: "detached" });
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

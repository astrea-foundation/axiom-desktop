import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

let browser: Browser;
let server: ViteDevServer;
let origin: string;
const captures = process.env.AXIOM_SIGN_IN_CAPTURE_DIR;
before(async () => {
  server = await createServer({ cacheDir: fileURLToPath(new URL("../../node_modules/.vite-tests/sign-in-status.test", import.meta.url)), configFile: false, root: fileURLToPath(new URL("../../src/renderer", import.meta.url)),
    plugins: [react(), tailwindcss()], server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
  await server.listen();
  origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
  if (captures) await mkdir(captures, { recursive: true });
});
after(async () => { await browser?.close(); await server?.close(); });

async function open(theme: string, width: number, scenario = "tls") {
  const page = await browser.newPage({ viewport: { width, height: 460 } });
  page.setDefaultTimeout(8000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/*", (route) => {
    const url = route.request().url();
    if (url === `${origin}?scenario=${scenario}`) return route.fulfill({ contentType: "text/html", body: `<!doctype html><html lang="en" data-theme="${theme}"><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module">import RefreshRuntime from "/@react-refresh"; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type; window.__vite_plugin_react_preamble_installed__ = true; await import("/@fs/${fileURLToPath(new URL("./fixtures/sign-in-status.tsx", import.meta.url))}");</script></body></html>` });
    return url.startsWith(origin) ? route.continue() : route.abort();
  });
  await page.goto(`${origin}?scenario=${scenario}`);
  await page.getByRole("region", { name: "Browser sign-in" }).waitFor();
  await page.evaluate(() => document.fonts.ready);
  return { page, errors };
}

for (const theme of ["light", "dark"]) for (const width of [480, 1120]) {
  test(`TLS sign-in failure explains recovery and discloses diagnostics (${theme}, ${width}px)`, async () => {
    const { page, errors } = await open(theme, width);
    try {
      const banner = page.getByRole("region", { name: "Browser sign-in" });
      await banner.getByText("Couldn’t connect securely", { exact: true }).waitFor();
      assert.match(await banner.innerText(), /browser sign-in could not start/);
      assert.match(await banner.innerText(), /VPN, proxy, or network filter/);
      assert.match(await banner.innerText(), /phone hotspot/);
      const diagnostic = banner.getByText(/^provider error:/);
      assert.equal(await diagnostic.isVisible(), false);
      if (captures) await page.screenshot({ path: `${captures}/${theme}-${width}.png` });

      // Native disclosure remains usable without a pointer.
      const disclosure = banner.locator("summary").filter({ hasText: "Technical details" });
      await disclosure.focus();
      await page.keyboard.press("Enter");
      await diagnostic.waitFor();
      assert.match(await diagnostic.innerText(), /client error \(Connect\): received corrupt message of type InvalidContentType$/);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true,
        "expanded diagnostics must wrap within the window");
      if (captures) await page.screenshot({ path: `${captures}/${theme}-${width}-details.png` });

      await banner.getByRole("button", { name: "Try again", exact: true }).click();
      await banner.getByText("Opening your browser…", { exact: true }).waitFor();
      assert.equal(await banner.locator("details").count(), 0);
      await banner.getByRole("button", { name: "Cancel sign-in", exact: true }).click();
      await page.getByText("Sign-in status dismissed", { exact: true }).waitFor();
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("an expired authorization keeps its own message and can be dismissed", async () => {
  const { page, errors } = await open("light", 1120, "expired");
  try {
    const banner = page.getByRole("region", { name: "Browser sign-in" });
    await banner.getByText("Sign-in didn’t finish", { exact: true }).waitFor();
    await banner.getByText("authorization expired", { exact: true }).waitFor();
    assert.equal(await banner.locator("details").count(), 0);
    await banner.getByRole("button", { name: "Dismiss sign-in status", exact: true }).click();
    await page.getByText("Sign-in status dismissed", { exact: true }).waitFor();
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

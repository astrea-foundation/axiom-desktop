import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium, type Browser, type Page } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { securityEvidence } from "../../../../fixtures/desktop-security.mts";

let browser: Browser;
let server: ViteDevServer;
let origin: string;
const captures = process.env.AXIOM_PRIVACY_CAPTURE_DIR;
before(async () => {
  server = await createServer({ configFile: false, root: fileURLToPath(new URL("../../src/renderer", import.meta.url)),
    plugins: [react(), tailwindcss()], server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
  await server.listen(); origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
  if (captures) await mkdir(captures, { recursive: true });
});
after(async () => { await browser?.close(); await server?.close(); });
async function open(theme = "light", mockClock = false) {
  const page = await browser.newPage({ viewport: { width: 1120, height: 800 }, bypassCSP: true });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.setDefaultTimeout(8000);
  if (mockClock) await page.clock.install({ time: new Date("2026-09-10T05:00:00Z") });
  // tsx's name helper must also exist inside serialized page callbacks.
  await page.addInitScript({ content: "window.__name = (fn) => fn;" });
  await page.route("**/*", (route) => {
    const url = route.request().url();
    if (url === origin) return route.fulfill({ contentType: "text/html", body: `<!doctype html><html data-theme="${theme}"><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module">import RefreshRuntime from "/@react-refresh"; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type; window.__vite_plugin_react_preamble_installed__ = true; await import("/@fs/${fileURLToPath(new URL("./fixtures/privacy-proof.tsx", import.meta.url))}");</script></body></html>` });
    if (url.startsWith(origin)) return route.continue();
    return route.abort();
  });
  await page.goto(origin);
  try { await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor(); }
  catch (error) { console.error("Privacy harness startup failed", errors, await page.locator("body").innerText()); await page.close(); throw error; }
  await page.evaluate(() => document.fonts.ready);
  return { page, errors };
}
const proofButton = (page: Page) => page.getByRole("button", { name: /View privacy proof$/ });
const dialog = (page: Page) => page.getByRole("dialog", { name: "Privacy proof", exact: true });
const count = (page: Page) => page.evaluate(() => (window as any).__privacyTest.calls());
const capture = async (page: Page, name: string) => { if (captures) await page.screenshot({ path: `${captures}/${name}.png` }); };

for (const theme of ["light", "dark"]) test(`privacy proof matches the composer and disclosures work (${theme})`, async () => {
  const { page, errors } = await open(theme);
  try {
    const width = (await proofButton(page).boundingBox())!.width;
    await proofButton(page).hover();
    assert.equal((await proofButton(page).boundingBox())!.width, width);
    await capture(page, `${theme}-badge`);
    await proofButton(page).click();
    await dialog(page).waitFor();
    assert.equal(await count(page), 0, "inspecting does not refresh");
    assert.equal(await page.locator("summary").filter({ hasText: "Hardware evidence" }).count(), 1);
    assert.equal(await page.getByText("UpToDate", { exact: true }).first().isVisible(), false);
    await capture(page, `${theme}-summary`);
    await page.locator("summary").filter({ hasText: "Hardware evidence" }).click();
    // Disclosures animate open; their content becomes visible within the first frames.
    await page.getByText("UpToDate", { exact: true }).first().waitFor({ state: "visible" });
    const compactWidth = (await dialog(page).boundingBox())!.width;
    await page.locator("summary").filter({ hasText: "Technical details" }).click();
    await page.getByText("a1b2c3d4".repeat(8), { exact: true }).waitFor({ state: "visible" });
    await page.waitForFunction((before) => {
      const bounds = document.querySelector<HTMLElement>("dialog[data-technical-open]")?.getBoundingClientRect();
      return bounds !== undefined && bounds.width >= before + 200;
    }, compactWidth);
    assert.ok((await dialog(page).boundingBox())!.width < 800, "the widened report stays bounded");
    await capture(page, `${theme}-details`);
    await page.locator("summary").filter({ hasText: "Workload manifest" }).click();
    await page.locator("textarea[data-chat-input]").evaluate((element: HTMLTextAreaElement) => element.focus());
    assert.equal(await dialog(page).evaluate((element) => element.contains(document.activeElement)), true, "modal prevents focusing the chat");
    await page.evaluate(() => { (window as any).__copied = null; Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: async (text: string) => { (window as any).__copied = text; } }); });
    await page.getByRole("button", { name: "Copy verification report", exact: true }).click();
    await page.getByText("Copied", { exact: true }).waitFor();
    const copied = await page.evaluate(() => JSON.parse((window as any).__copied));
    assert.equal(copied.format, "axiom-verification-report-v1");
    assert.equal(copied.evidence.modelId, "test-model");
    assert.doesNotMatch(JSON.stringify(copied), /quiet place|timeline|messages|credential/);
    await page.keyboard.press("Escape");
    await dialog(page).waitFor({ state: "detached" });
    assert.equal(await proofButton(page).evaluate((element) => document.activeElement === element), true);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("refresh is separate, deduplicated, and leaves inspection accessible during verification", async () => {
  const { page, errors } = await open();
  try {
    const refresh = page.getByRole("button", { name: "Refresh TEE verification", exact: true });
    await refresh.click();
    assert.equal(await dialog(page).count(), 0);
    assert.equal(await count(page), 1);
    assert.equal(await refresh.isDisabled(), true);
    await proofButton(page).click();
    await dialog(page).waitFor();
    assert.equal(await page.getByRole("button", { name: "Checking…", exact: true }).isDisabled(), true);
    await capture(page, "checking");
    await page.evaluate(() => (window as any).__privacyTest.finish());
    await page.getByRole("button", { name: "Refresh", exact: true }).waitFor();
    assert.equal(await count(page), 1);
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    assert.equal(await count(page), 2);
    await page.evaluate(() => (window as any).__privacyTest.finish(true));
    await page.getByRole("alert").waitFor();
    assert.match(await dialog(page).innerText(), /TEE check failed/);
    assert.match(await dialog(page).innerText(), /Previous report checked/);
    await capture(page, "failed");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("expiry, missing evidence, and provisional replies stay explicit without an approved-list notice", async () => {
  const { page, errors } = await open();
  try {
    await page.evaluate(() => (window as any).__privacyTest.setDisabledReason("Reconnect to refresh verification"));
    await page.evaluate((evidence) => (window as any).__privacyTest.update({ securityEvidence: evidence }),
      { ...securityEvidence(), hardExpiresAtUnixSeconds: Math.floor(Date.now() / 1000) + 1, approvedWorkloadIdentity: false, workloadIdentityAllowlistConfigured: false });
    await page.getByRole("button", { name: "TEE proof expired. View privacy proof", exact: true }).waitFor();
    await proofButton(page).click();
    assert.match(await dialog(page).innerText(), /expired/);
    assert.doesNotMatch(await dialog(page).innerText(), /Software identity was not checked|Software identity is not approved/);
    await capture(page, "expired");
    await page.evaluate(() => (window as any).__privacyTest.update({ securityEvidence: null }));
    await page.getByText("No report available for this model.").waitFor();
    assert.equal(await page.getByRole("button", { name: "Copy verification report" }).isDisabled(), true);
    await page.evaluate(() => { (window as any).__privacyTest.update({ running: true }); (window as any).__privacyTest.setWeb(true); });
    assert.match(await dialog(page).innerText(), /Latest reply verification\s+Pending/);
    assert.match(await dialog(page).innerText(), /not end-to-end encrypted/);
    assert.equal(await page.getByRole("button", { name: "Refresh", exact: true }).isDisabled(), true);
    await capture(page, "missing-pending-web");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("idle proof renews before expiry without opening the dialog or duplicating an in-flight check", async () => {
  const { page, errors } = await open("dark", true);
  try {
    await page.clock.fastForward(239_000);
    assert.equal(await count(page), 0);
    await page.clock.fastForward(2_000);
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 1);
    assert.equal(await dialog(page).count(), 0);
    await page.clock.fastForward(120_000);
    assert.equal(await count(page), 1, "only one verification may be pending");
    await page.evaluate(() => (window as any).__privacyTest.finish());
    await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(239_000);
    assert.equal(await count(page), 1, "the next renewal uses the new report's expiry");
    await page.clock.fastForward(2_000);
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 2);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("idle renewal waits for replies, connection, sign-in, and thread resync", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.evaluate(() => (window as any).__privacyTest.update({ running: true }));
    await page.clock.fastForward(301_000);
    assert.equal(await count(page), 0);
    await page.getByRole("button", { name: "TEE proof expired. View privacy proof", exact: true }).waitFor();
    await page.evaluate(() => {
      (window as any).__privacyTest.setDisabledReason("Reconnect to refresh verification");
      (window as any).__privacyTest.update({ running: false });
    });
    await page.clock.fastForward(120_000);
    assert.equal(await count(page), 0);
    await page.evaluate(() => (window as any).__privacyTest.setDisabledReason("Sign in to refresh verification"));
    await page.clock.fastForward(120_000);
    assert.equal(await count(page), 0);
    await page.evaluate(() => {
      (window as any).__privacyTest.update({ needsResync: true });
      (window as any).__privacyTest.setDisabledReason(undefined);
    });
    await page.clock.fastForward(120_000);
    assert.equal(await count(page), 0);
    await page.evaluate(() => (window as any).__privacyTest.update({ needsResync: false }));
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 1);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("failed idle renewal stays failed and backs off, while manual refresh remains available", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.clock.fastForward(241_000);
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    await page.evaluate(() => (window as any).__privacyTest.finish(true));
    await page.getByRole("button", { name: "TEE check failed. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(29_000);
    assert.equal(await count(page), 1);
    await page.clock.fastForward(2_000);
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 2);
    await page.evaluate(() => (window as any).__privacyTest.finish(true));
    await page.getByRole("button", { name: "TEE check failed. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(59_000);
    assert.equal(await count(page), 2, "a second failure doubles the retry interval");
    await page.clock.fastForward(2_000);
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 3);
    await page.evaluate(() => (window as any).__privacyTest.finish(true));
    await page.getByRole("button", { name: "TEE check failed. View privacy proof", exact: true }).waitFor();
    await page.getByRole("button", { name: "Refresh TEE verification", exact: true }).click();
    assert.equal(await count(page), 4, "the user can retry before the automatic backoff ends");
    await page.evaluate(() => (window as any).__privacyTest.finish());
    await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor();
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("returning the same short lease cannot trigger a refresh loop or extend its validity", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.evaluate((evidence) => (window as any).__privacyTest.update({ securityEvidence: {
      ...evidence, verifiedAtUnixSeconds: Math.floor(Date.now() / 1000), hardExpiresAtUnixSeconds: Math.floor(Date.now() / 1000) + 10,
    } }), securityEvidence());
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    await page.evaluate(() => (window as any).__privacyTest.finish(false, true));
    await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(29_000);
    assert.equal(await count(page), 1);
    await page.getByRole("button", { name: "TEE proof expired. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(2_000);
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 2);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("wake checks overdue evidence immediately and unmount cancels scheduled renewals", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.clock.setSystemTime(new Date("2026-09-10T05:10:00Z"));
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 1);
    await page.evaluate(() => (window as any).__privacyTest.setShowChat(false));
    await proofButton(page).waitFor({ state: "detached" });
    await page.evaluate(() => (window as any).__privacyTest.finish());
    await page.clock.fastForward(600_000);
    await page.evaluate(() => {
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("online"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    assert.equal(await count(page), 1, "an unmounted badge does not schedule or react to wake events");
    await page.evaluate(() => (window as any).__privacyTest.setShowChat(true));
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 2, "returning to an expired thread renews it");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("initial evidence is automatic while development and an existing preflight are left alone", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.evaluate(() => (window as any).__privacyTest.update({ securityEvidence: null, security: { state: "unattested_development" } }));
    await page.clock.fastForward(600_000);
    assert.equal(await count(page), 0);
    await page.evaluate(() => (window as any).__privacyTest.update({ security: { state: "verifying" }, securityVerificationPending: true }));
    await page.clock.fastForward(600_000);
    assert.equal(await count(page), 0);
    await page.evaluate(() => (window as any).__privacyTest.update({ security: { state: "unverified" }, securityVerificationPending: false }));
    await page.getByRole("button", { name: "Verifying TEE. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 1);
    await page.evaluate(() => (window as any).__privacyTest.finish());
    await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor();
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("dialog stays on screen and contains keyboard focus at narrow sizes", async () => {
  const { page, errors } = await open("dark");
  try {
    await page.setViewportSize({ width: 540, height: 600 });
    await proofButton(page).click();
    await dialog(page).waitFor();
    for (let index = 0; index < 15; index++) {
      await page.keyboard.press("Tab");
      assert.equal(await dialog(page).evaluate((element) => element.contains(document.activeElement)), true);
    }
    await page.locator("summary").filter({ hasText: "Technical details" }).click();
    await capture(page, "narrow-dark");
    const bounds = (await dialog(page).boundingBox())!;
    assert.ok(bounds.x >= 0 && bounds.y >= 0 && bounds.x + bounds.width <= 540 && bounds.y + bounds.height <= 600);
    assert.equal(await dialog(page).evaluate((element) => element.scrollWidth <= element.clientWidth), true);
    // Changing models remounts the badge and closes its old report immediately.
    await page.evaluate(() => (window as any).__privacyTest.setModel("other-model"));
    await dialog(page).waitFor({ state: "detached" });
    await proofButton(page).click();
    assert.doesNotMatch(await dialog(page).innerText(), /Hardware evidence/);
    await page.evaluate(() => (window as any).__privacyTest.setScope("account-two:runtime-two"));
    await dialog(page).waitFor({ state: "detached" });
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("Phala separates gateway, workers, and receipt policy, with safe clipboard failure", async () => {
  const { page, errors } = await open();
  try {
    const report = { ...securityEvidence(), providerId: "phala", attestationProtocol: "phala-aci-tdx-v1", e2eeProtocol: "phala-aci-v2",
      workloadManifest: null, tlsSpkiFingerprint: null,
      checks: [["Gateway TDX", "UpToDate"], ["Worker TEE", "verified locally"], ["Worker GPU", "verified locally"],
        ["Encrypted channel", "nonce-bound"], ["Signed receipt", "required"]].map(([name, detail]) => ({ name: name!, detail: detail!, passed: true })),
      providerClaims: [{ name: "preverified_worker_sessions", value: "2" }, { name: "provider_statement", value: "A provider assertion is not a local verification result." }],
    };
    await page.evaluate((evidence) => (window as any).__privacyTest.update({ securityEvidence: evidence }), report);
    await proofButton(page).click();
    assert.match(await dialog(page).innerText(), /phala/);
    await page.locator("summary").filter({ hasText: "Hardware evidence" }).click();
    await page.getByText("Gateway TDX", { exact: true }).first().waitFor({ state: "visible" });
    assert.match(await dialog(page).innerText(), /Gateway TDX/);
    assert.match(await dialog(page).innerText(), /Worker GPU/);
    await page.locator("summary").filter({ hasText: "Technical details" }).click();
    await page.getByText("Receipt policy", { exact: true }).waitFor({ state: "visible" });
    assert.match(await dialog(page).innerText(), /Receipt policy\s+Required for each reply/);
    assert.match(await dialog(page).innerText(), /Latest reply verification\s+No reply yet/);
    assert.match(await dialog(page).innerText(), /preverified worker sessions/);
    await capture(page, "phala-details");
    await page.evaluate(() => Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: async () => { throw new Error("Clipboard denied"); } }));
    await page.getByRole("button", { name: "Copy verification report", exact: true }).click();
    await page.getByRole("alert").waitFor();
    assert.match(await dialog(page).innerText(), /Couldn’t copy the report/);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

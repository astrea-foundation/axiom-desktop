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
  if (mockClock) {
    // Installation alone still advances with real time. Freeze before loading
    // the fixture so slow page startup cannot consume the renewal boundary.
    await page.clock.install({ time: new Date("2026-09-10T04:59:00Z") });
    await page.clock.pauseAt(new Date("2026-09-10T05:00:00Z"));
  }
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

for (const theme of ["light", "dark"]) test(`outdated TEE requires a click and keeps a warning after consent (${theme})`, async () => {
  const { page, errors } = await open(theme);
  try {
    await page.evaluate(() => (window as any).__privacyTest.update({ security: { state: "outdated" }, securityEvidence: null }));
    const notice = page.locator("[data-outdated-tee-warning]");
    await notice.getByRole("button", { name: "Continue generation" }).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__privacyTest.acceptances()), 0);
    assert.match(await notice.innerText(), /until Axiom restarts/);
    await capture(page, `${theme}-outdated-consent`);
    await notice.getByRole("button", { name: "Continue generation" }).click();
    await page.getByRole("button", { name: "TEE updates needed. View privacy proof", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__privacyTest.acceptances()), 1);
    assert.equal(await notice.getByRole("button").count(), 0);
    assert.match(await notice.innerText(), /You chose to continue/);
    await capture(page, `${theme}-outdated-accepted`);
    await page.evaluate(() => (window as any).__privacyTest.update({ security: { state: "verifying" }, securityVerificationPending: true }));
    await page.getByRole("button", { name: "TEE updates needed. View privacy proof", exact: true }).waitFor();
    assert.match(await notice.innerText(), /You chose to continue/);
    await page.evaluate(() => {
      // Keep the injected failed state observable instead of racing automatic
      // renewal, which has separate tests below.
      (window as any).__privacyTest.setDisabledReason("Renewal paused for failed-state assertion");
      (window as any).__privacyTest.update({ security: { state: "failed" }, securityVerificationPending: false });
    });
    await notice.waitFor({ state: "detached" });
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

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
    assert.equal(await page.getByText("UpToDate", { exact: true }).first().isVisible(), true);
    await page.locator("summary").filter({ hasText: "Technical details" }).click();
    assert.equal(await page.getByText("a1b2c3d4".repeat(8), { exact: true }).isVisible(), true);
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

test("TEE help supports hover, keyboard, clicks and narrow dialogs without refreshing proof", async () => {
  const { page, errors } = await open();
  try {
    await proofButton(page).click();
    const help = page.getByRole("button", { name: "What does TEE verified mean?", exact: true });
    const tooltip = page.getByRole("tooltip");
    await help.hover();
    await tooltip.waitFor();
    assert.equal(await tooltip.innerText(), "What does TEE verified mean?\n\nA TEE (Trusted Execution Environment) is a protected space on the provider’s servers. Axiom checks security evidence from the server’s hardware before sending your encrypted messages there. This helps keep your messages private while the AI processes them.");
    await tooltip.hover();
    assert.equal(await tooltip.isVisible(), true, "help stays open while reading it");
    await page.getByRole("heading", { name: "Privacy proof", exact: true }).hover();
    await tooltip.waitFor({ state: "hidden" });

    await help.focus();
    await tooltip.waitFor();
    assert.equal(await help.getAttribute("aria-describedby"), await tooltip.getAttribute("id"));
    await page.keyboard.press("Escape");
    await tooltip.waitFor({ state: "hidden" });
    assert.equal(await dialog(page).isVisible(), true, "Escape dismisses help before the report");
    assert.equal(await help.evaluate((element) => element === document.activeElement), true);
    await page.keyboard.press("Enter");
    await tooltip.waitFor();
    await page.keyboard.press("Tab");
    await tooltip.waitFor({ state: "hidden" });

    await page.setViewportSize({ width: 360, height: 640 });
    await help.click();
    await tooltip.waitFor();
    const helpBounds = (await tooltip.boundingBox())!;
    const reportBounds = (await dialog(page).boundingBox())!;
    assert.ok(helpBounds.x >= reportBounds.x && helpBounds.x + helpBounds.width <= reportBounds.x + reportBounds.width);
    assert.ok(helpBounds.y + helpBounds.height <= reportBounds.y + reportBounds.height);
    await page.getByRole("heading", { name: "Privacy proof", exact: true }).click();
    await tooltip.waitFor({ state: "hidden" });
    assert.equal(await count(page), 0, "explaining verification must not request new evidence");
    await page.evaluate(() => (window as any).__privacyTest.update({ security: { state: "failed" } }));
    await help.waitFor({ state: "detached" });
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
      { ...securityEvidence(), hardExpiresAtUnixSeconds: Math.floor(Date.now() / 1000) + 1 });
    await page.getByRole("button", { name: "Report needs refresh. View privacy proof", exact: true }).waitFor();
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

for (const theme of ["light", "dark"]) test(`a streaming reply outlives its report and only authenticated completion verifies it (${theme})`, async () => {
  const { page, errors } = await open(theme, true);
  const answer = { id: "answer", turnId: "turn", kind: "assistant", text: "A reply that takes longer than the report lease.", status: "in_progress" };
  try {
    await page.evaluate((item) => (window as any).__privacyTest.update({ running: true, timeline: [item] }), answer);
    await page.getByRole("button", { name: "Verifying reply. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(301_000);
    await page.getByRole("button", { name: "Verifying reply. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 0, "report refresh must not overlap the stream");
    await capture(page, `streaming-expired-${theme}`);
    await page.setViewportSize({ width: 540, height: 600 });
    const badgeBounds = (await proofButton(page).boundingBox())!;
    assert.ok(badgeBounds.x >= 0 && badgeBounds.x + badgeBounds.width <= 540, "pending label stays visible in a narrow composer");
    await capture(page, `streaming-narrow-${theme}`);
    await page.setViewportSize({ width: 1120, height: 800 });
    await proofButton(page).click();
    assert.match(await dialog(page).innerText(), /Latest reply verification\s+Pending/);
    assert.match(await dialog(page).innerText(), /Cached attestation report\s+Report needs refresh/);
    await page.locator("summary").filter({ hasText: "Latest reply verification" }).click();
    assert.match(await dialog(page).innerText(), /stays provisional until its complete response is authenticated/);
    await capture(page, `streaming-report-${theme}`);
    await page.evaluate((item) => {
      (window as any).__privacyTest.setDisabledReason("Reconnect to refresh verification");
      (window as any).__privacyTest.update({ running: false, timeline: [{ ...item, status: "completed" }] });
    }, answer);
    await page.getByRole("button", { name: "Reply incomplete. View privacy proof", exact: true }).waitFor();
    await page.evaluate((item) => (window as any).__privacyTest.update({ timeline: [{ ...item, status: "completed", terminalVerified: true }] }), answer);
    await page.getByRole("button", { name: "Reply verified. View privacy proof", exact: true }).waitFor();
    assert.match(await dialog(page).innerText(), /Latest reply verification\s+Verified/);
    assert.match(await dialog(page).innerText(), /Cached attestation report\s+Report needs refresh/);
    await capture(page, `verified-expired-report-${theme}`);
    await page.evaluate(() => (window as any).__privacyTest.setDisabledReason(undefined));
    assert.equal(await count(page), 0, "completing a reply does not automatically renew proof");
    await dialog(page).getByRole("button", { name: "Refresh", exact: true }).click();
    await dialog(page).getByRole("status").filter({ hasText: "Verifying TEE" }).waitFor();
    await page.getByRole("button", { name: "Reply verified. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 1);
    await page.evaluate(() => (window as any).__privacyTest.finish(true));
    await dialog(page).getByRole("status").filter({ hasText: "TEE check failed" }).waitFor();
    await page.getByRole("button", { name: "Reply verified. View privacy proof", exact: true }).waitFor();
    assert.match(await dialog(page).innerText(), /Latest reply verification\s+Verified/);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("interrupted and failed output cannot inherit a previous reply's verification", async () => {
  const { page, errors } = await open("dark", true);
  const previous = { id: "previous", turnId: "old-turn", kind: "assistant", text: "Earlier verified reply.", status: "completed", terminalVerified: true };
  const question = { id: "question", turnId: "new-turn", kind: "user", text: "Continue", status: "completed" };
  try {
    await page.evaluate((item) => (window as any).__privacyTest.update({ timeline: [item] }), previous);
    await page.getByRole("button", { name: "Reply verified. View privacy proof", exact: true }).waitFor();
    await page.evaluate((items) => (window as any).__privacyTest.update({ timeline: items }), [previous, question]);
    await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor();
    for (const status of ["in_progress", "cancelled", "failed"]) {
      await page.evaluate((items) => (window as any).__privacyTest.update({ timeline: items }), [previous, question,
        { id: "partial", turnId: "new-turn", kind: "assistant", text: "Partial output", status }]);
      const label = status === "failed" ? "Reply check failed" : "Reply incomplete";
      await page.getByRole("button", { name: `${label}. View privacy proof`, exact: true }).waitFor();
      await proofButton(page).click();
      assert.match(await dialog(page).innerText(), status === "failed" ? /Latest reply verification\s+Failed/ : /Latest reply verification\s+Incomplete/);
      await page.getByRole("button", { name: "Close privacy proof", exact: true }).click();
    }
    assert.equal(await count(page), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("expiry and unrelated activity never start background verification", async () => {
  const { page, errors } = await open("dark", true);
  try {
    await page.clock.fastForward(299_000);
    assert.equal(await count(page), 0);
    await page.clock.fastForward(2_000);
    await page.getByRole("button", { name: "Report needs refresh. View privacy proof", exact: true }).waitFor();
    await page.evaluate(() => {
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("online"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await page.keyboard.press("Shift");
    await page.clock.fastForward(600_000);
    assert.equal(await count(page), 0);
    await page.evaluate(() => (window as any).__privacyTest.update({ securityEvidence: null, security: { state: "unverified" } }));
    await page.getByRole("button", { name: "Verify TEE. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(60_000);
    assert.equal(await count(page), 0, "mounting an unverified thread is not composition");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("manual refresh remains available after expiry and failure, without automatic retries", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.clock.fastForward(301_000);
    await page.getByRole("button", { name: "Refresh TEE verification", exact: true }).click();
    assert.equal(await count(page), 1);
    await page.evaluate(() => (window as any).__privacyTest.finish(true));
    await page.getByRole("button", { name: "TEE check failed. View privacy proof", exact: true }).waitFor();
    await page.clock.fastForward(600_000);
    assert.equal(await count(page), 1);
    await page.getByRole("button", { name: "Refresh TEE verification", exact: true }).click();
    await page.evaluate(() => (window as any).__privacyTest.finish());
    await page.getByRole("button", { name: "TEE verified. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 2);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("idle and wake update the displayed proof without making requests", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.clock.fastForward(1_800_000);
    await page.getByRole("button", { name: "Idle. View privacy proof", exact: true }).waitFor();
    await capture(page, "idle-light");
    await page.keyboard.press("Shift");
    await page.getByRole("button", { name: "Report needs refresh. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 0);
    await page.evaluate(() => (window as any).__privacyTest.setShowChat(false));
    await page.clock.fastForward(600_000);
    await page.evaluate(() => (window as any).__privacyTest.setShowChat(true));
    await page.getByRole("button", { name: "Report needs refresh. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("long replies stay active and completing them does not start a verification", async () => {
  const { page, errors } = await open("light", true);
  try {
    await page.evaluate(() => (window as any).__privacyTest.update({ running: true }));
    await page.clock.fastForward(1_860_000);
    await page.getByRole("button", { name: "Verifying reply. View privacy proof", exact: true }).waitFor();
    await page.evaluate(() => (window as any).__privacyTest.update({ running: false }));
    await page.getByRole("button", { name: "Report needs refresh. View privacy proof", exact: true }).waitFor();
    assert.equal(await count(page), 0);
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

test("NEAR shows hardware and incomplete reply status, with safe clipboard failure", async () => {
  const { page, errors } = await open();
  try {
    const report = { ...securityEvidence(), attestationProtocol: "near-tdx-nvidia-v2", e2eeProtocol: "near-v3" };
    await page.evaluate((evidence) => (window as any).__privacyTest.update({ securityEvidence: evidence }), report);
    await proofButton(page).click();
    assert.match(await dialog(page).innerText(), /NEAR/);
    await page.locator("summary").filter({ hasText: "Hardware evidence" }).click();
    assert.match(await dialog(page).innerText(), /Intel TDX/);
    assert.match(await dialog(page).innerText(), /NVIDIA GPU/);
    await page.locator("summary").filter({ hasText: "Technical details" }).click();
    assert.match(await dialog(page).innerText(), /Latest reply verification\s+No reply yet/);
    await capture(page, "near-details");
    await page.evaluate(() => Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: async () => { throw new Error("Clipboard denied"); } }));
    await page.getByRole("button", { name: "Copy verification report", exact: true }).click();
    await page.getByRole("alert").waitFor();
    assert.match(await dialog(page).innerText(), /Couldn’t copy the report/);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

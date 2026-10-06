import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

let browser: Browser, server: ViteDevServer, origin: string;
before(async () => {
  server = await createServer({configFile: false, root: fileURLToPath(new URL("../../", import.meta.url)), plugins: [react(), tailwindcss()],
    server: {host: "127.0.0.1", port: 0}, logLevel: "error"});
  await server.listen(); origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({headless: true});
});
after(async () => { await browser?.close(); await server?.close(); });

async function open() {
  const page = await browser.newPage({viewport: {width: 800, height: 1000}});
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/*", (route) => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await page.addInitScript({content: "window.__name = (fn) => fn;"});
  await page.addInitScript(() => {
    const records: Record<string, any[]> = {};
    const requests: any[] = [];
    let gate: (() => void) | null = null;
    let held = false, fail = false;
    Object.assign(window, {__cryptoTest: {requests, records,
      hold: () => { held = true; }, release: () => { held = false; gate?.(); },
      failOnce: () => { fail = true; },
      finish: (account: string) => { records[account].forEach((p) => { p.status = "finished"; p.credited_microusd = p.amount_microusd; }); },
    }, axiomDesktop: {agent: {
      cryptoOptions: async () => ({options: {enabled: true, zcash_discount_bps: 500, min_amount_microusd: 5_000_000,
        max_amount_microusd: 1_000_000_000, currencies: [
          {code: "btc", name: "Bitcoin", network: "btc"}, {code: "eth", name: "Ethereum", network: "eth"},
          {code: "ltc", name: "Litecoin", network: "ltc"}, {code: "sol", name: "Solana", network: "sol"},
          {code: "usdc", name: "USD Coin (Ethereum)", network: "eth"},
          {code: "usdttrc20", name: "USDT", network: "TRON (TRC20)"}, {code: "xmr", name: "Monero", network: "xmr"},
        ]}}),
      cryptoPayments: async (account: string) => ({payments: structuredClone(records[account] ?? [])}),
      createCryptoPayment: async (request: any, account: string) => {
        requests.push({...request, account});
        if (fail) { fail = false; throw new Error("test lost response"); }
        const payment = {id: request.id, status: "waiting", amount_microusd: request.amountMicrousd, credited_microusd: 0,
          pay_currency: request.payCurrency, pay_amount: "25.00000001", pay_address: "TtestPaymentAddress", payin_extra_id: "0012345",
          expires_at: new Date(Date.now() + 60_000).toISOString(), created_at: new Date().toISOString(), review_required: false};
        (records[account] ??= []).push(payment);
        if (held) await new Promise<void>((resolve) => { gate = resolve; });
        return {payment};
      },
    }}});
  });
  await page.goto(`${origin}tests/browser/crypto-funding.fixture.html`);
  try { await page.getByRole("button", {name: "Other crypto", exact: true}).waitFor(); }
  catch (error) { console.error(errors, await page.locator("body").innerText()); await page.close(); throw error; }
  return {page, errors};
}

test("primary discount, safe retries, quote, network, memo and status restoration", async () => {
  const {page, errors} = await open();
  try {
    assert.match(await page.locator("body").innerText(), /5% off · \$9\.50 buys \$10 credit/);
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.getByRole("combobox", {name: "Crypto", exact: true}).click();
    await page.getByRole("option", {name: /^USDT/}).click();
    const networkWarning = page.getByRole("alert").filter({hasText: "Funds sent on other networks may be permanently lost."});
    assert.match(await networkWarning.innerText(), /Network: TRON \(TRC20\) only\./);
    await page.evaluate(() => (window as any).__cryptoTest.failOnce());
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.getByRole("button", {name: "Retry", exact: true}).click();
    await page.getByRole("status").filter({hasText: /^Awaiting payment$/}).waitFor();
    const requests = await page.evaluate(() => (window as any).__cryptoTest.requests);
    assert.equal(requests[0].id, requests[1].id);
    assert.equal(requests[0].amountMicrousd, 25_000_000);
    assert.equal(await page.getByRole("button", {name: "Copy memo"}).count(), 1);
    assert.match(await page.locator("body").innerText(), /Use TRON \(TRC20\)/);
    assert.equal(await page.getByRole("img", {name: "Crypto payment address"}).count(), 1);
    assert.match(await networkWarning.innerText(), /Network: TRON \(TRC20\) only\./);
    await page.screenshot({path: "/tmp/axiom-nowpayments-desktop-quote.png", fullPage: true});
    await page.getByRole("button", {name: "Close deposit", exact: true}).click();
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.evaluate(() => (window as any).__cryptoTest.finish("account-a"));
    await page.getByRole("status").filter({hasText: /^Credited$/}).waitFor();
    assert.equal(await page.getByRole("button", {name: "Copy memo"}).count(), 0);
    assert.deepEqual(errors, []);
    await page.screenshot({path: "/tmp/axiom-nowpayments-desktop.png", fullPage: true});
  } finally { await page.close(); }
});

test("an in-flight payment reply cannot enter another account", async () => {
  const {page, errors} = await open();
  try {
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.getByRole("combobox", {name: "Crypto", exact: true}).click();
    await page.getByRole("option", {name: /^USDT/}).click();
    await page.evaluate(() => (window as any).__cryptoTest.hold());
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.evaluate(() => window.dispatchEvent(new Event("axiom-test-switch-account")));
    await page.evaluate(() => (window as any).__cryptoTest.release());
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.getByRole("button", {name: "Create payment", exact: true}).waitFor();
    assert.equal(await page.getByText("TtestPaymentAddress", {exact: true}).count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("icon picker uses clean names and keyboard selection; Escape closes one layer and restores focus", async () => {
  const {page, errors} = await open();
  try {
    const entry = page.getByRole("button", {name: "Other crypto", exact: true});
    assert.equal(await entry.locator("img").count(), 3);
    await entry.click();
    const dialog = page.getByRole("dialog", {name: "Deposit crypto", exact: true});
    await dialog.waitFor();
    assert.equal(await page.getByRole("button", {name: "Create payment", exact: true}).isDisabled(), true);
    const picker = page.getByRole("combobox", {name: "Crypto", exact: true});
    await picker.click();
    const options = page.getByRole("option");
    assert.equal(await options.count(), 7);
    assert.equal(await options.locator("img").count(), 7);
    assert.equal(await page.getByRole("option", {name: "Bitcoin", exact: true}).count(), 1);
    await picker.press("End");
    await picker.press("Enter");
    assert.equal(await picker.innerText(), "Monero");
    await picker.press("ArrowDown");
    await picker.press("Escape");
    assert.equal(await dialog.count(), 1);
    assert.equal(await page.getByRole("listbox").count(), 0);
    await picker.press("Escape");
    await dialog.waitFor({state: "detached"});
    assert.equal(await entry.evaluate((element) => element === document.activeElement), true);
    await entry.click();
    assert.equal(await picker.innerText(), "Monero");
    await picker.click();
    await page.getByRole("option", {name: "Ethereum", exact: true}).click();
    assert.match(await dialog.getByRole("alert").innerText(), /Network: Ethereum only\./);
    await picker.click();
    await page.getByRole("option", {name: "Bitcoin", exact: true}).click();
    assert.equal(await dialog.getByRole("alert").count(), 0);
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.getByRole("img", {name: "Crypto payment address"}).waitFor();
    await page.getByRole("button", {name: "Close deposit", exact: true}).click();
    await entry.click();
    assert.equal(await page.getByRole("img", {name: "Crypto payment address"}).count(), 1);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("deposit dialog and icon picker fit a small window and dark theme", async () => {
  const {page, errors} = await open();
  try {
    await page.setViewportSize({width: 360, height: 568});
    await page.evaluate(() => document.documentElement.dataset.theme = "dark");
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    const picker = page.getByRole("combobox", {name: "Crypto", exact: true});
    await picker.click();
    const bounds = await page.getByRole("listbox").boundingBox();
    assert.ok(bounds && bounds.x >= 0 && bounds.y >= 0 && bounds.x + bounds.width <= 360 && bounds.y + bounds.height <= 568);
    await page.getByRole("option", {name: /^USDC/}).click();
    const networkWarning = page.getByRole("alert").filter({hasText: "Funds sent on other networks may be permanently lost."});
    assert.match(await networkWarning.innerText(), /Network: Ethereum only\./);
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.getByRole("img", {name: "Crypto payment address"}).waitFor();
    assert.match(await networkWarning.innerText(), /Network: Ethereum only\./);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

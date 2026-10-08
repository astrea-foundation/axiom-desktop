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
    let held = false, fail = false, lose = false;
    Object.assign(window, {__cryptoTest: {requests, records,
      hold: () => { held = true; }, release: () => { held = false; gate?.(); },
      failOnce: () => { fail = true; },
      loseOnce: () => { lose = true; },
      progress: (account: string, status: string) => { records[account].forEach((p) => { p.status = status; }); },
      finish: (account: string) => { records[account].forEach((p) => { p.status = "finished"; p.credited_microusd = p.amount_microusd; }); },
      finishOriginal: (account: string) => { const p = records[account][0]; p.status = "finished"; p.credited_microusd = p.amount_microusd; },
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
        const existing = records[account]?.find((p) => p.id === request.id);
        if (existing) return {payment: structuredClone(existing)};
        const index = records[account]?.length ?? 0;
        const payment = {id: request.id, status: "waiting", amount_microusd: request.amountMicrousd, credited_microusd: 0,
          pay_currency: request.payCurrency, pay_amount: `25.${String(index + 1).padStart(8, "0")}`, pay_address: `TtestPaymentAddress${index || ""}`, payin_extra_id: "0012345",
          expires_at: new Date(Date.now() + 60_000).toISOString(), created_at: new Date().toISOString(), review_required: false};
        (records[account] ??= []).push(payment);
        if (held) await new Promise<void>((resolve) => { gate = resolve; });
        if (lose) { lose = false; throw new Error("test lost attached response"); }
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

test("rate countdown renews automatically; cancelling retains the original payment", async () => {
  const {page, errors} = await open();
  try {
    const time = new Date("2026-10-06T07:00:00Z");
    await page.clock.install({time});
    await page.clock.pauseAt(time);
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    const picker = page.getByRole("combobox", {name: "Crypto", exact: true});
    await picker.click();
    await page.getByRole("option", {name: "Bitcoin", exact: true}).click();
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    const countdown = page.getByRole("timer", {name: "Time left to send for this rate"});
    assert.equal(await countdown.innerText(), "01:00");
    await page.clock.runFor(15_000);
    assert.equal(await countdown.innerText(), "00:45");
    await page.getByRole("button", {name: "Cancel deposit", exact: true}).click();
    await page.getByRole("button", {name: "Create payment", exact: true}).waitFor();
    assert.equal(await picker.evaluate((element) => element === document.activeElement), true);
    assert.equal(await page.getByRole("img", {name: "Crypto payment address"}).count(), 0);
    assert.match(await page.getByRole("status").filter({hasText: /^Deposit closed\./}).innerText(), /Deposit closed\. Funds already sent will still be processed\./);
    const original = await page.evaluate(() => (window as any).__cryptoTest.records["account-a"][0]);
    assert.equal(original.status, "waiting");
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    assert.equal(await countdown.innerText(), "01:00");
    const requests = await page.evaluate(() => (window as any).__cryptoTest.requests);
    assert.equal(requests.length, 2);
    assert.notEqual(requests[0].id, requests[1].id);
    await page.clock.runFor(60_000);
    await page.getByText("Rate updated. Use this amount and address.", {exact: true}).waitFor();
    assert.equal(await countdown.innerText(), "01:00");
    assert.match(await page.getByRole("dialog").innerText(), /25\.00000003/);
    assert.match(await page.getByLabel("Crypto address", {exact: true}).innerText(), /TtestPaymentAddress2/);
    const renewedRequests = await page.evaluate(() => (window as any).__cryptoTest.requests);
    assert.equal(renewedRequests.length, 3);
    assert.equal(renewedRequests[2].amountMicrousd, 25_000_000);
    assert.equal(renewedRequests[2].payCurrency, "btc");
    assert.notEqual(renewedRequests[1].id, renewedRequests[2].id);
    await page.evaluate(() => (window as any).__cryptoTest.finish("account-a"));
    await page.clock.runFor(5_000);
    await page.getByRole("status").filter({hasText: /^Credited$/}).waitFor();
    const records = await page.evaluate(() => (window as any).__cryptoTest.records["account-a"]);
    assert.equal(records.length, 3);
    assert.equal(records[0].credited_microusd, 25_000_000);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("a lost renewal response retries the same order and never loops on an expired rate", async () => {
  const {page, errors} = await open();
  try {
    const time = new Date("2026-10-06T07:00:00Z");
    await page.clock.install({time}); await page.clock.pauseAt(time);
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.getByRole("combobox", {name: "Crypto", exact: true}).click();
    await page.getByRole("option", {name: "Bitcoin", exact: true}).click();
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.getByRole("timer").waitFor();
    await page.evaluate(() => (window as any).__cryptoTest.loseOnce());
    await page.clock.runFor(60_000);
    await page.getByRole("alert").filter({hasText: "Couldn’t refresh the rate."}).waitFor();
    assert.equal(await page.getByRole("timer").count(), 0);
    assert.equal(await page.getByRole("dialog").getByRole("button", {name: "Copy address"}).count(), 0);
    await page.clock.runFor(15_000);
    assert.equal(await page.evaluate(() => (window as any).__cryptoTest.requests.length), 2);
    await page.getByRole("button", {name: "Retry quote", exact: true}).click();
    await page.getByText("Rate updated. Use this amount and address.", {exact: true}).waitFor();
    const requests = await page.evaluate(() => (window as any).__cryptoTest.requests);
    assert.equal(requests.length, 3);
    assert.equal(requests[1].id, requests[2].id);
    assert.equal(await page.evaluate(() => (window as any).__cryptoTest.records["account-a"].length), 2);
    assert.equal(await page.getByRole("timer").innerText(), "00:45");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("sent transfers survive reopening and keep confirming beyond the quote deadline", async () => {
  const {page, errors} = await open();
  try {
    const time = new Date("2026-10-06T07:00:00Z");
    await page.clock.install({time}); await page.clock.pauseAt(time);
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.getByRole("combobox", {name: "Crypto", exact: true}).click();
    await page.getByRole("option", {name: "Monero", exact: true}).click();
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.getByRole("button", {name: "I’ve sent it", exact: true}).click();
    await page.clock.runFor(75_000);
    await page.getByRole("status").filter({hasText: /^Checking payment$/}).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__cryptoTest.requests.length), 1);
    await page.evaluate(() => window.dispatchEvent(new Event("axiom-test-remount-funding")));
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.locator("summary").filter({hasText: "Recent payments"}).click();
    await page.getByRole("button", {name: /\$25\.00 · XMR/}).click();
    await page.getByRole("status").filter({hasText: /^Checking payment$/}).waitFor();
    assert.equal(await page.getByRole("timer").count(), 0);
    await page.evaluate(() => (window as any).__cryptoTest.progress("account-a", "confirming"));
    await page.clock.runFor(5_000);
    await page.getByRole("status").filter({hasText: /^Confirming$/}).waitFor();
    await page.clock.runFor(600_000);
    assert.equal(await page.evaluate(() => (window as any).__cryptoTest.requests.length), 1);
    await page.evaluate(() => (window as any).__cryptoTest.finish("account-a"));
    await page.clock.runFor(5_000);
    await page.getByRole("status").filter({hasText: /^Credited$/}).waitFor();
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("late renewal replies cannot replace a cancelled checkout, another account or a confirming transfer", async () => {
  for (const action of ["cancel", "switch", "confirming"]) {
    const {page, errors} = await open();
    try {
      const time = new Date("2026-10-06T07:00:00Z");
      await page.clock.install({time}); await page.clock.pauseAt(time);
      await page.getByRole("button", {name: "Other crypto", exact: true}).click();
      await page.getByRole("combobox", {name: "Crypto", exact: true}).click();
      await page.getByRole("option", {name: "Bitcoin", exact: true}).click();
      await page.getByRole("button", {name: "Create payment", exact: true}).click();
      await page.getByRole("timer").waitFor();
      await page.evaluate(() => (window as any).__cryptoTest.hold());
      await page.clock.runFor(60_000);
      await page.getByRole("status").filter({hasText: /^Updating rate…$/}).waitFor();
      if (action === "cancel") await page.getByRole("button", {name: "Cancel deposit", exact: true}).click();
      else if (action === "switch") await page.evaluate(() => window.dispatchEvent(new Event("axiom-test-switch-account")));
      else {
        await page.evaluate(() => (window as any).__cryptoTest.progress("account-a", "confirming"));
        await page.clock.runFor(5_000);
        await page.getByRole("status").filter({hasText: /^Confirming$/}).waitFor();
      }
      await page.evaluate(() => (window as any).__cryptoTest.release());
      if (action === "switch") await page.getByRole("button", {name: "Other crypto", exact: true}).click();
      if (action === "confirming") {
        await page.evaluate(() => (window as any).__cryptoTest.finishOriginal("account-a"));
        await page.clock.runFor(5_000);
        await page.getByRole("status").filter({hasText: /^Credited$/}).waitFor();
      } else await page.getByRole("button", {name: "Create payment", exact: true}).waitFor();
      assert.equal(await page.getByRole("timer").count(), 0);
      assert.equal(await page.getByRole("img", {name: "Crypto payment address"}).count(), 0);
      const requests = await page.evaluate(() => (window as any).__cryptoTest.requests);
      assert.equal(requests.length, 2);
      assert.equal(requests[1].account, "account-a");
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  }
});

test("provider confirmations stop renewal even when the customer has not marked the transfer sent", async () => {
  const {page, errors} = await open();
  try {
    const time = new Date("2026-10-06T07:00:00Z");
    await page.clock.install({time}); await page.clock.pauseAt(time);
    await page.getByRole("button", {name: "Other crypto", exact: true}).click();
    await page.getByRole("combobox", {name: "Crypto", exact: true}).click();
    await page.getByRole("option", {name: "Monero", exact: true}).click();
    await page.getByRole("button", {name: "Create payment", exact: true}).click();
    await page.getByRole("timer").waitFor();
    await page.evaluate(() => (window as any).__cryptoTest.progress("account-a", "confirming"));
    await page.clock.runFor(75_000);
    await page.getByRole("status").filter({hasText: /^Confirming$/}).waitFor();
    assert.equal(await page.getByRole("timer").count(), 0);
    assert.equal(await page.getByRole("button", {name: "I’ve sent it", exact: true}).count(), 0);
    assert.equal(await page.evaluate(() => (window as any).__cryptoTest.requests.length), 1);
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

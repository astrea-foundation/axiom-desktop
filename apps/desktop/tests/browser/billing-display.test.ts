import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";
import { chromium, type Browser, type Page } from "playwright";

const root = fileURLToPath(new URL("../../", import.meta.url)).replaceAll("\\", "/");
const repository = resolve(root, "../..").replaceAll("\\", "/");

// The real Desktop usage host runs against a scripted native API: `plan` opens a
// weekly window, `spent` posts dollar charges. $0.63 trial plus $10.00 added.
const code = `
  import React from 'react';
  import {createRoot} from 'react-dom/client';
  import {UsagePanel} from '/@fs/${repository}/packages/chat-ui/src/UsagePanel.tsx';
  import {AccountMenu} from '/@fs/${repository}/packages/chat-ui/src/AccountMenu.tsx';
  import {ChatHostProvider} from '/@fs/${repository}/packages/chat-ui/src/ChatHost.tsx';
  import {desktopCreditUsage} from '/@fs/${root}src/renderer/src/creditUsageHost.ts';
  import '/@fs/${root}src/renderer/src/index.css';
  const query = new URLSearchParams(location.search);
  const plan = query.get('plan') === '1', spent = query.get('spent') !== '0';
  const credits = {format:'credits@3',scale:'1000000',revision:plan?'7':'0',paid_fallback:true,payment_review_required:false,subscription:null,
    purchased:{currency:'usd',scale:'100000000',posted_units:'1000000000',held_units:'0',available_units:'1000000000'},
    trial:{posted_units:'632000000',held_units:'0'},
    allowance:plan?{limit_units:'10000000000',remaining_units:'4200000000',held_units:'0',resets_at:'2026-11-23T00:00:00+00:00',effective_until:'2026-12-01T00:00:00+00:00',generation:'window'}
      :{limit_units:'0',remaining_units:'0',held_units:'0',resets_at:null,effective_until:null,generation:null}};
  const creditUsage = period => plan ? {period,scale:'1000000',total_units:'1500000',allocation:{subscription_units:'1000000',trial_units:'500000'},
      models:[{provider:'near',model_id:'first',units:'1000000'},{provider:'near',model_id:'second',units:'500000'}],
      purchased:{currency:'usd',scale:'100000000',total_units:'0',models:[]}}
    : {period,scale:'1000000',total_units:'0',allocation:{subscription_units:'0',trial_units:'0'},models:[],
      purchased:{currency:'usd',scale:'100000000',total_units:'0',models:[]}};
  const requests = window.__usageRequests = [];
  const agent = {creditBilling: async action => { requests.push('credits:' + action.kind);
    return {result: action.kind === 'summary' ? {kind:'summary',value:credits} : {kind:'usage',value:creditUsage(action.period)}}; }};
  const host = {
    creditUsageSummary: (account, request) => desktopCreditUsage(agent, account, request),
    usageSummary: async (_account, request) => { requests.push('dollars');
      return {summary: spent ? {period:request.period,totalCostMicrousd:'370000',models:[
        {provider:'near',modelId:'first',modelName:'First model',costMicrousd:'250000'},{provider:'near',modelId:'second',modelName:null,costMicrousd:'120000'}]}
        : {period:request.period,totalCostMicrousd:'0',models:[]}}; },
  };
  const billing = {revision:1,ledgerSequence:1,currency:'microUSD',postedMicrousd:10632000,availableMicrousd:10632000,trialMicrousd:632000,paidMicrousd:10000000,credits};
  const noop = () => {};
  createRoot(document.getElementById('root')).render(React.createElement(ChatHostProvider,{host},
    React.createElement(UsagePanel,{accountId:'display-test',connected:true,onLogin:noop}),
    React.createElement('div',{style:{maxWidth:260,marginTop:24}},
      React.createElement(AccountMenu,{account:{kind:'valid',title:'Tester',detail:'Connected securely'},accountId:'display-test',billing,connected:true,
        onRefreshBalance:async()=>{},onSignIn:noop,onLogout:noop,onOpenSettings:noop,onOpenBalance:noop}))));
`;

let server: ViteDevServer, browser: Browser, origin: string;
before(async () => {
  server = await createServer({configFile: false, root, logLevel: "error", plugins: [react(), tailwind(), {
    name: "billing-display-test",
    resolveId(id) { if (id === "/billing-display-test.js") return "\0billing-display-test"; },
    load(id) { if (id === "\0billing-display-test") return code; },
    configureServer(server) {
      server.middlewares.use(async (request, response, next) => {
        if (!request.url?.startsWith("/billing-display-test?")) return next();
        const html = await server.transformIndexHtml(request.url, '<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><main id="root" style="max-width:760px;margin:auto;padding:20px"></main><script type="module" src="/billing-display-test.js"></script></body></html>');
        response.setHeader("Content-Type", "text/html"); response.end(html);
      });
    },
  }], server: {host: "127.0.0.1", port: 0}});
  await server.listen();
  origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({headless: true});
});
after(async () => { await browser?.close(); await server?.close(); });

async function open(query: string, width = 800): Promise<{ page: Page; errors: string[] }> {
  const page = await browser.newPage({viewport: {width, height: 1000}});
  page.setDefaultTimeout(8_000);
  const errors: string[] = []; page.on("pageerror", error => errors.push(error.message));
  await page.route("**/*", route => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await page.clock.install({time: new Date("2026-11-21T12:00:00Z")});
  await page.goto(`${origin}billing-display-test?${query}`);
  return { page, errors };
}

async function billingRow(page: Page): Promise<string> {
  await page.getByRole("button", {name: "Account menu", exact: true}).click();
  const text = (await page.getByRole("menuitem", {name: /^Billing/}).innerText()).replace(/\s+/g, " ").trim();
  await page.keyboard.press("Escape");
  return text;
}

test("an account without a plan sees dollar spending and a dollar balance", async () => {
  for (const width of [800, 390]) {
    const { page, errors } = await open("plan=0&spent=1", width);
    try {
      const total = page.locator("[data-usage-total]");
      await total.getByText("$0.37", {exact: true}).waitFor();
      await page.getByText("Spent all time", {exact: true}).waitFor();
      await page.getByRole("tab", {name: "This week", exact: true}).click();
      await page.getByText("Spent this week", {exact: true}).waitFor();
      await total.getByText("$0.37", {exact: true}).waitFor();
      // The all-zero credit report never replaces the dollar charges, and no credit switch is offered.
      assert.equal(await page.getByText("0 credits", {exact: true}).count(), 0);
      assert.equal(await page.getByRole("button", {name: "Allowance credits", exact: true}).count(), 0);
      assert.equal(await page.getByRole("button", {name: "Added credit", exact: true}).count(), 0);
      await page.getByRole("button", {name: /^First model, near, \$0\.25,/}).waitFor();
      const freshness = page.locator("[data-usage-freshness]");
      assert.equal(await freshness.innerText(), "Updated just now. Replies are counted as soon as they finish.");
      await page.clock.runFor(120_000);
      assert.equal(await freshness.innerText(), "Updated 2 min ago. Replies are counted as soon as they finish.");
      await page.getByRole("button", {name: "Refresh usage", exact: true}).click();
      await page.getByText("Updated just now. Replies are counted as soon as they finish.", {exact: true}).waitFor();
      assert.deepEqual(await page.evaluate(() => (window as any).__usageRequests.slice(-3)), ["credits:summary", "credits:usage", "dollars"]);
      assert.equal(await billingRow(page), "Billing $10.63");
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  }
});

test("an empty period says when replies will appear", async () => {
  const { page, errors } = await open("plan=0&spent=0");
  try {
    await page.getByRole("heading", {name: "No spending yet", exact: true}).waitFor();
    await page.getByText("Replies appear here as soon as they finish.", {exact: true}).waitFor();
    assert.equal(await page.locator("[data-usage-total]").innerText(), "$0.00");
    await page.getByRole("tab", {name: "This week", exact: true}).click();
    await page.getByRole("heading", {name: "No spending this week", exact: true}).waitFor();
    await page.getByText("Replies appear here as soon as they finish.", {exact: true}).waitFor();
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("a plan holder keeps the credit view and sees weekly credits beside the dollar balance", async () => {
  const { page, errors } = await open("plan=1&spent=1");
  try {
    const total = page.locator("[data-usage-total]");
    await total.getByText("1.5 credits", {exact: true}).waitFor();
    await page.getByRole("button", {name: "Allowance credits", exact: true}).waitFor();
    await page.getByRole("button", {name: "Added credit", exact: true}).waitFor();
    assert.equal(await page.locator("[data-usage-freshness]").innerText(), "Updated just now. Replies are counted as soon as they finish.");
    // The dollar report is not requested while weekly credits apply.
    assert.equal(await page.evaluate(() => (window as any).__usageRequests.includes("dollars")), false);
    assert.equal(await billingRow(page), "Billing 4,200 credits · $10.63");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

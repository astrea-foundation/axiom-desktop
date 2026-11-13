import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { createServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";
import { chromium } from "playwright";

test("purchased-dollar usage stays separate and selected across period changes and refresh", async () => {
  const root = fileURLToPath(new URL("../../", import.meta.url));
  const repository = resolve(root, "../..").replaceAll("\\", "/");
  const code = `
    import React from 'react';
    import {createRoot} from 'react-dom/client';
    import {UsagePanel} from '/@fs/${repository}/packages/chat-ui/src/UsagePanel.tsx';
    import {ChatHostProvider} from '/@fs/${repository}/packages/chat-ui/src/ChatHost.tsx';
    import '/@fs/${root.replaceAll("\\", "/")}src/renderer/src/index.css';
    const host={creditUsageSummary:async (_account,request)=>({period:request.period,scale:'1000000',
      total_units:'1500000',allocation:{subscription_units:'1000000',trial_units:'500000'},
      models:[{provider:'near',model_id:'first',units:'1000000'},{provider:'near',model_id:'second',units:'500000'}],
      purchased:{currency:'usd',scale:'100000000',total_units:'125000000',
        models:[{provider:'near',model_id:'first',units:'75000000'},{provider:'near',model_id:'second',units:'50000000'}]}})};
    createRoot(document.getElementById('root')).render(React.createElement(ChatHostProvider,{host},
      React.createElement(UsagePanel,{accountId:'money-test',connected:true,onLogin:()=>{}})));
  `;
  const server = await createServer({configFile: false, root, logLevel: "error", plugins: [react(), tailwind(), {
    name: "purchased-usage-test",
    resolveId(id) { if (id === "/purchased-usage-test.js") return "\0purchased-usage-test"; },
    load(id) { if (id === "\0purchased-usage-test") return code; },
    configureServer(server) {
      server.middlewares.use(async (request, response, next) => {
        if (request.url !== "/purchased-usage-test") return next();
        const html = await server.transformIndexHtml(request.url, '<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><main id="root" style="max-width:760px;margin:auto;padding:20px"></main><script type="module" src="/purchased-usage-test.js"></script></body></html>');
        response.setHeader("Content-Type", "text/html"); response.end(html);
      });
    },
  }], server: {host: "127.0.0.1", port: 0}});
  let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
  try {
    await server.listen();
    const origin = server.resolvedUrls!.local[0]!;
    browser = await chromium.launch({headless: true});
    for (const width of [800, 390]) {
      const page = await browser.newPage({viewport: {width, height: 1000}});
      try {
        const errors: string[] = []; page.on("pageerror", error => errors.push(error.message));
        await page.route("**/*", route => route.request().url().startsWith(origin) ? route.continue() : route.abort());
        await page.goto(origin + "purchased-usage-test");
        const total = page.locator("[data-usage-total]");
        await total.getByText("1.5 credits", {exact: true}).waitFor();
        await page.getByRole("button", {name: "Purchased credit", exact: true}).click();
        await total.getByText("$1.25", {exact: true}).waitFor();
        await page.getByRole("tab", {name: "This month", exact: true}).click();
        await total.getByText("$1.25", {exact: true}).waitFor();
        await page.getByRole("button", {name: "Refresh usage", exact: true}).click();
        await total.getByText("$1.25", {exact: true}).waitFor();
        assert.equal(await page.getByRole("button", {name: "Purchased credit", exact: true}).getAttribute("aria-pressed"), "true");
        await page.getByRole("button", {name: "Allowance credits", exact: true}).click();
        await total.getByText("1.5 credits", {exact: true}).waitFor();
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
  } finally { await browser?.close(); await server.close(); }
});

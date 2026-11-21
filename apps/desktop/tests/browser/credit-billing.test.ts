import assert from "node:assert/strict";
import { before, after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
let browser: Browser, server: ViteDevServer, origin: string;
before(async () => {
  server = await createServer({configFile:false,root:fileURLToPath(new URL("../../",import.meta.url)),cacheDir:fileURLToPath(new URL("../../node_modules/.vite-credit-billing",import.meta.url)),plugins:[react(),tailwindcss()],server:{host:"127.0.0.1",port:0},logLevel:"error"});
  await server.listen(); origin = server.resolvedUrls!.local[0]!; browser = await chromium.launch({headless:true});
});
after(async () => {await browser?.close(); await server?.close();});
async function open(clock = false, initiallyHidden = false, balanceOnly = false) {
  const page = await browser.newPage({viewport:{width:800,height:1000}});
  await page.addInitScript({content:"window.__name = fn => fn;"});
  if (clock) await page.clock.install();
  if (initiallyHidden) await page.addInitScript(() => {
    let visible = false;
    Object.defineProperty(document, "visibilityState", {configurable:true,get:()=>visible ? "visible" : "hidden"});
    (window as any).__showBilling = () => {visible = true; document.dispatchEvent(new Event("visibilitychange"));};
  });
  page.setDefaultTimeout(10000);
  const errors: string[] = []; page.on("pageerror", e => errors.push(e.message));
  page.on("response", response => {if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`);});
  await page.route("**/*", route => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await page.addInitScript(balanceOnly => {
    const calls: any[] = []; let gate: (() => void) | null = null, held = false, subscription: any = null;
    let balance = balanceOnly ? "1200000000" : "800000000";
    const standard = {key:"standard",name:"Standard",amount_cents:1000,weekly_units:"5000000000",version:"plans-v3"};
    let usageUnits = "0", failure: {kind: string; message: string} | null = null;
    let auto: any = {currency:"usd",scale:"100000000",available:true,enabled:false,version:"0",threshold_units:"500000000",amount_cents:1000,monthly_cap_cents:3000,payment_method_id:null,paused_reason:null,cap_month:"2026-11",used_cents:0,consent_version:"auto-topup-v2"};
    Object.assign(window, {__billingTest:{calls,hold:()=>{held=true;},release:()=>{held=false;gate?.();},
      usage:(units:string)=>{usageUnits=units;},
      fail:(kind:string | null, message = `Could not load ${kind}`)=>{failure=kind ? {kind, message} : null;},
      subscription:(state:string)=>{subscription={id:"20000000-0000-4000-8000-000000000001",plan_key:"starter",amount_cents:250,weekly_units:"1250000000",state,paid_through_at:"2026-11-01T00:00:00Z",cancel_at_period_end:false,pending_plan_key:null};}},axiomDesktop:{agent:{creditBilling:async (action:any, account:string) => {
      calls.push({action:structuredClone(action),account});
      let kind = action.kind, value: any;
      if (failure && kind === failure.kind) throw new Error(failure.message);
      switch (kind) {
        case "summary": value = {format:"credits@3",enabled:true,scale:"1000000",revision:"1",policy_version:"credits-v4",reference_microusd_per_credit:1000,default_multiplier_ppm:1000000,paid_fallback:true,payment_review_required:false,
          purchased:{currency:"usd",scale:"100000000",posted_units:account==="account-a"?balance:"100000000",held_units:"0",available_units:account==="account-a"?balance:"100000000"},trial:{posted_units:"0",held_units:"0"},
          allowance:{limit_units:"1250000000",remaining_units:"482500000",held_units:"0",generation:null,resets_at:"2027-01-08T00:00:00Z",effective_until:"2027-01-08T00:00:00Z"},subscription,
          capabilities:{subscriptions:!balanceOnly,auto_topup:!balanceOnly,card:!balanceOnly,crypto:true,admin:false}}; break;
        case "card_options": value={enabled:!balanceOnly,currency:"usd",min_amount_microusd:5000000,max_amount_microusd:1000000000}; break;
        case "balance_plan_options":
          value = balanceOnly ? {
            catalog: {enabled:true,currency:"usd",billing_interval:"month",allowance_interval_days:7,plans:[standard]},
            balance_subscription_id: subscription?.id ?? null, renewal_consent_version:"balance-renew-v1",
            renewal_authorized:Boolean(subscription), renewal_failure:null,
            coverage_end_on: subscription ? null : "2027-01-08", completion_reserved_units:"0",
          } : null;
          break;
        case "purchase_balance_plan":
          if (!balanceOnly) throw new Error("Unexpected balance purchase");
          balance = "200000000";
          subscription = {id:action.id,plan_key:standard.key,amount_cents:standard.amount_cents,weekly_units:standard.weekly_units,
            state:"active",paid_through_at:"2027-01-08T12:00:00Z",cancel_at_period_end:false,pending_plan_key:null};
          kind = "balance_plan_purchase";
          value = {id:action.id,subscription_id:action.id,plan_key:standard.key,amount_cents:1000,
            period_start:"2026-12-08T12:00:00Z",period_end:"2027-01-08T12:00:00Z"};
          break;
        case "set_balance_plan_renewal":
          if (!balanceOnly || subscription?.id !== action.id) throw new Error("Unexpected renewal request");
          subscription.cancel_at_period_end = !action.enabled;
          kind = "balance_plan_renewal";
          value = {id:action.id,enabled:action.enabled,paid_through_at:subscription.paid_through_at};
          break;
        case "plans": value={enabled:true,currency:"usd",billing_interval:"month",allowance_interval_days:7,plans:[
          {key:"starter",name:"Starter",amount_cents:250,weekly_units:"1250000000",version:"plans-v3"},
          {key:"standard",name:"Standard",amount_cents:1000,weekly_units:"5000000000",version:"plans-v3"},
          {key:"plus",name:"Plus",amount_cents:2500,weekly_units:"12500000000",version:"plans-v3"},
          {key:"max",name:"Max",amount_cents:10000,weekly_units:"50000000000",version:"plans-v3"}]}; break;
        case "cards": value={cards:[{id:"pm_Fixture",brand:"visa",last4:"4242",exp_month:12,exp_year:2030}]}; break;
        case "auto_topup": value=auto; break;
        case "configure_auto_topup": auto={...auto,...action.settings,version:String(Number(auto.version)+1)}; value=auto; kind="auto_topup"; break;
        case "operations": case "card_payments": value=[]; break;
        case "usage": value={period:"all_time",scale:"1000000",total_units:usageUnits,allocation:{subscription_units:usageUnits,trial_units:"0"},purchased:{currency:"usd",scale:"100000000",total_units:"0",models:[]},models:usageUnits === "0" ? [] : [{provider:"near",model_id:"fixture",units:usageUnits}]}; break;
        case "checkout": value={id:action.id,kind:"subscription",state:"pending",url:"https://checkout.stripe.com/c/pay/subscription",amount_cents:250,card:null,created_at:"2026-11-13T00:00:00Z"}; kind="operation"; break;
        case "create_card_payment":
          if (held) await new Promise<void>(resolve=>{gate=resolve;});
          value={id:action.id,amount_microusd:action.amount_microusd,credited_microusd:0,refunded_microusd:0,status:"open",checkout_url:"https://checkout.stripe.com/c/pay/fixture",expires_at:"2027-01-08T00:00:00Z",created_at:"2027-01-01T00:00:00Z",review_required:false,cancel_requested:false}; kind="card_payment"; break;
        default: throw new Error("Unexpected action: "+kind);
      }
      return {result:{kind,value:structuredClone(value)}};
    }}}});
  }, balanceOnly);
  await page.goto(origin+"tests/browser/credit-billing.fixture.html");
  if (balanceOnly) {
    await page.getByRole("button", {name:"Choose plan"}).waitFor();
    return {page, errors};
  }
  try {await page.getByRole("button",{name:"Continue with card"}).waitFor();}
  catch (error) {
    throw new Error(JSON.stringify({errors,body:await page.locator("body").innerText(),state:await page.evaluate(() => ({visibility:document.visibilityState,mock:!!(window as any).__billingTest,calls:(window as any).__billingTest?.calls}))}),{cause:error});
  }
  await page.locator('details[aria-label="Automatic top-up"] > summary').click();
  await page.getByRole("button",{name:"Enable automatic top-up",exact:true}).waitFor();
  return {page,errors};
}
async function alignedPlanCards(page: import("playwright").Page) {
  const cards = await page.locator('.subscription-plan-grid > div').evaluateAll(cards => cards.map(card => ({
    row: Math.round(card.getBoundingClientRect().top),
    lines: [...card.children].map(line => line.getBoundingClientRect().top),
  })));
  for (const card of cards) for (const other of cards.filter(other => other.row === card.row)) {
    assert(card.lines.every((line, index) => Math.abs(line - other.lines[index]!) < 1), "plan names, prices, credit lines and buttons share row positions");
  }
}
test("native credit controls separate balances and require explicit saved-card charge consent", async () => {
  const {page,errors}=await open();
  try {
    assert.match(await page.locator("body").innerText(),/Weekly credits.*482.*Added credit.*\$8\.00/s);
    assert.match(await page.locator("body").innerText(),/1,250 credits \/ week/);
    for (const total of ["5,000", "20,000", "50,000", "200,000"]) await page.getByText(`${total} credits/month`, {exact:true}).waitFor();
    await alignedPlanCards(page);
    assert(!(await page.locator('section[aria-label="Subscription plans"]').innerText()).includes('(4 weeks)'));
    if (process.env.AXIOM_UI_ARTIFACT_DIR) await page.locator('section[aria-label="Subscription plans"]').screenshot({path:`${process.env.AXIOM_UI_ARTIFACT_DIR}/native-monthly-plan-totals.png`});
    await page.setViewportSize({width:320,height:1000});
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "monthly plan totals fit a narrow screen");
    for (const total of ["5,000", "20,000", "50,000", "200,000"]) await page.getByText(`${total} credits/month`, {exact:true}).waitFor();
    await alignedPlanCards(page);
    await page.setViewportSize({width:800,height:1000});
    assert.equal(await page.getByLabel("Balance threshold (USD)",{exact:true}).inputValue(), "5.00");
    assert(await page.getByRole("button",{name:"Enable automatic top-up",exact:true}).isDisabled());
    const initial=await page.evaluate(()=>(window as any).__billingTest.calls);
    assert(!initial.some((c:any)=>["checkout","create_card_payment","configure_auto_topup"].includes(c.action.kind)));
    await page.getByLabel("Balance threshold (USD)",{exact:true}).fill("100.25");
    await page.getByLabel("Monthly cap (USD)",{exact:true}).fill("3000");
    await page.getByLabel("Saved card",{exact:true}).selectOption("pm_Fixture");
    await page.getByLabel(/I authorize automatic card charges/).check();
    await page.getByRole("button",{name:"Enable automatic top-up",exact:true}).click();
    await page.getByRole("button",{name:"Turn off",exact:true}).waitFor();
    const saved=await page.evaluate(()=>(window as any).__billingTest.calls.find((c:any)=>c.action.kind==="configure_auto_topup"));
    assert.equal(saved.action.settings.threshold_units,"10025000000"); assert.equal(saved.action.settings.monthly_cap_cents,300000);
    assert.equal(saved.action.settings.consent_accepted,true); assert.equal(saved.account,"account-a");
    await page.getByRole("button",{name:"Crypto",exact:true}).click(); await page.getByText("Choose a cryptocurrency",{exact:true}).waitFor();
    assert.deepEqual(errors,[]);
  } finally {await page.close();}
});

for (const state of ["expired", "canceled"]) test(`an ${state} subscription starts a new checkout for the same plan`, async () => {
  const {page,errors}=await open();
  try {
    await page.evaluate(state => {(window as any).__billingTest.subscription(state); window.dispatchEvent(new Event("focus"));}, state);
    await page.getByText(`Subscription: ${state}`,{exact:true}).waitFor();
    await page.getByRole("button",{name:"Subscribe",exact:true}).first().click();
    await page.getByRole("dialog",{name:"Confirm subscription plan"}).getByText("Monthly total: 5,000 credits.",{exact:true}).waitFor();
    await page.getByRole("button",{name:"Confirm",exact:true}).click();
    await page.getByText("Complete your subscription in the browser, then return here.",{exact:true}).waitFor();
    const actions=await page.evaluate(()=>(window as any).__billingTest.calls.map((call:any)=>call.action));
    assert.equal(actions.filter((a:any)=>a.kind==="checkout").length,1);
    assert(!actions.some((a:any)=>a.kind==="change"));
    assert.deepEqual(errors,[]);
  } finally {await page.close();}
});

test("automatic billing refresh stays below the read budget", async () => {
  const {page}=await open(true);
  try {
    for (let n=0;n<2;n++) {
      await page.clock.runFor(30_001);
      await page.waitForFunction(expected=>(window as any).__billingTest.calls.filter((c:any)=>c.action.kind==="operations").length>=expected,n+2);
    }
    const reads=await page.evaluate(()=>(window as any).__billingTest.calls.filter((c:any)=>["summary","card_options","card_payments","usage","plans","auto_topup","cards","operations"].includes(c.action.kind)).length);
    assert(reads<=26,`expected at most 26 reads in a minute, got ${reads}`);
  } finally {await page.close();}
});

test("billing loads initially while hidden and refreshes on visibility", async () => {
  const {page,errors}=await open(false,true);
  try {
    assert.equal(await page.evaluate(()=>document.visibilityState),"hidden");
    const initial=await page.evaluate(()=>(window as any).__billingTest.calls.filter((c:any)=>c.action.kind==="operations").length);
    assert(initial>=1);
    await page.evaluate(()=>(window as any).__showBilling());
    await page.waitForFunction(initial=>(window as any).__billingTest.calls.filter((c:any)=>c.action.kind==="operations").length>initial,initial);
    assert.deepEqual(errors,[]);
  } finally {await page.close();}
});

test("usage refreshes independently of card history and reports its own failure", async () => {
  const {page, errors} = await open(true);
  try {
    await page.getByText("Usage", {exact:true}).click();
    await page.getByText("Weekly & trial credits used · 0", {exact:true}).waitFor();
    await page.evaluate(() => {
      (window as any).__billingTest.usage("2500000");
      (window as any).__billingTest.fail("card_payments");
    });
    await page.clock.runFor(30_001);
    await page.getByText("Could not load card_payments", {exact:true}).waitFor();
    const total = page.getByText("Weekly & trial credits used · 2.5", {exact:true});
    await total.waitFor();

    await page.evaluate(() => (window as any).__billingTest.fail("usage"));
    await page.clock.runFor(30_001);
    await page.getByText("Couldn’t refresh usage.", {exact:true}).waitFor();
    assert.equal(await total.count(), 0);
    await page.getByRole("button", {name:"Continue with card"}).click();
    await page.getByText(/Complete your payment in the browser/).waitFor();
    assert.deepEqual(errors, []);
  } finally {await page.close();}
});

test("billing refresh keeps a failed checkout visible until the next action", async () => {
  const {page, errors} = await open(true);
  try {
    const message = "The credit card processor is temporarily unavailable.";
    await page.evaluate(message => (window as any).__billingTest.fail("create_card_payment", message), message);
    const checkout = page.getByRole("button", {name:"Continue with card"});
    const alert = page.getByRole("alert").filter({hasText:message});
    await checkout.click();
    await alert.waitFor();
    await page.clock.runFor(30_001);
    await page.waitForFunction(() => (window as any).__billingTest.calls.filter((c:any) => c.action.kind === "operations").length >= 2);
    assert(await alert.isVisible());
    await page.evaluate(() => (window as any).__billingTest.fail(null));
    await checkout.click();
    await page.getByText(/Complete your payment in the browser/).waitFor();
    assert.equal(await alert.count(), 0);
    assert.deepEqual(errors, []);
  } finally {await page.close();}
});

test("Desktop subscribes from balance and manages automatic renewal with Stripe disabled", async () => {
  const {page, errors} = await open(false, false, true);
  try {
    await page.getByRole("button", {name:"Choose plan", exact:true}).click();
    const confirmation = page.getByRole("dialog", {name:"Confirm subscription plan"});
    await confirmation.getByText("Balance after purchase", {exact:true}).waitFor();
    await confirmation.getByText("$2.00", {exact:true}).waitFor();
    await confirmation.getByText("Jan 8, 2027 (UTC)", {exact:true}).waitFor();
    const confirmPayment = confirmation.getByRole("button", {name:"Pay $10.00 and subscribe", exact:true});
    assert(await confirmPayment.isDisabled());
    await confirmation.getByRole("checkbox", {name:/I authorize Axiom/}).check();
    if (process.env.AXIOM_UI_ARTIFACT_DIR) {
      await confirmation.screenshot({path:`${process.env.AXIOM_UI_ARTIFACT_DIR}/desktop-balance-auto-renewal.png`});
    }
    await confirmPayment.click();
    await page.getByText("Standard is active. Your next renewal is Jan 8, 2027 (UTC).", {exact:true}).waitFor();
    await page.getByRole("button", {name:"Cancel renewal", exact:true}).click();
    await page.getByRole("dialog", {name:"Cancel balance renewal"}).getByRole("button", {name:"Turn off renewal", exact:true}).click();
    await page.getByText("Plan ends Jan 8, 2027 (UTC). Automatic renewal is off.", {exact:true}).waitFor();
    await page.getByRole("button", {name:"Resume renewal", exact:true}).click();
    await page.getByRole("dialog", {name:"Resume balance renewal"}).getByRole("button", {name:"Resume renewal", exact:true}).click();
    await page.getByRole("button", {name:"Cancel renewal", exact:true}).waitFor();
    assert.equal(await page.getByRole("button", {name:"Manage payment method or cancel"}).count(), 0);
    const actions = await page.evaluate(() => (window as any).__billingTest.calls.map((entry:any) => entry.action));
    assert(!actions.some((action:any) => action.kind === "checkout" || action.kind === "create_card_payment"));
    const purchases = actions.filter((action:any) => action.kind === "purchase_balance_plan");
    assert.equal(purchases.length, 1);
    assert.equal(purchases[0].coverage_end_on, "2027-01-08");
    assert.equal(purchases[0].renewal_consent_version, "balance-renew-v1");
    assert.equal(purchases[0].renewal_consent_accepted, true);
    assert.deepEqual(actions.filter((action:any) => action.kind === "set_balance_plan_renewal").map((action:any) => action.enabled), [false, true]);
    assert.deepEqual(errors, []);
  } finally {await page.close();}
});

test("a late native card result cannot update the next account's billing screen", async () => {
  const {page,errors}=await open();
  try {
    await page.evaluate(()=>(window as any).__billingTest.hold());
    await page.getByRole("button",{name:"Continue with card"}).click();
    await page.getByRole("button",{name:"Switch account",exact:true}).click();
    await page.locator('details[aria-label="Automatic top-up"] > summary').click();
    await page.getByRole("button",{name:"Enable automatic top-up",exact:true}).waitFor();
    await page.evaluate(()=>(window as any).__billingTest.release());
    await page.waitForTimeout(100);
    assert.match(await page.locator("body").innerText(),/Added credit\s+\$1\.00/);
    assert.doesNotMatch(await page.locator("body").innerText(),/Complete your payment in the browser/);
    const charges=await page.evaluate(()=>(window as any).__billingTest.calls.filter((c:any)=>c.action.kind==="create_card_payment"));
    assert.equal(charges.length,1); assert.equal(charges[0].account,"account-a");
    assert.deepEqual(errors,[]);
  } finally {await page.close();}
});

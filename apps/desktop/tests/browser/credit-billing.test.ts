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
async function open(clock = false, initiallyHidden = false) {
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
  await page.addInitScript(() => {
    const calls: any[] = []; let gate: (() => void) | null = null, held = false, subscription: any = null;
    let auto: any = {currency:"usd",scale:"100000000",available:true,enabled:false,version:"0",threshold_units:"500000000",amount_cents:1000,monthly_cap_cents:3000,payment_method_id:null,paused_reason:null,cap_month:"2026-11",used_cents:0,consent_version:"auto-topup-v2"};
    Object.assign(window, {__billingTest:{calls,hold:()=>{held=true;},release:()=>{held=false;gate?.();},
      subscription:(state:string)=>{subscription={id:"20000000-0000-4000-8000-000000000001",plan_key:"starter",amount_cents:250,weekly_units:"125000000",state,paid_through_at:"2026-11-01T00:00:00Z",cancel_at_period_end:false,pending_plan_key:null};}},axiomDesktop:{agent:{creditBilling:async (action:any, account:string) => {
      calls.push({action:structuredClone(action),account});
      let kind = action.kind, value: any;
      switch (kind) {
        case "summary": value = {format:"credits@3",enabled:true,scale:"1000000",revision:"1",policy_version:"credits-v3",reference_microusd_per_credit:10000,default_multiplier_ppm:1000000,paid_fallback:true,payment_review_required:false,
          purchased:{currency:"usd",scale:"100000000",posted_units:account==="account-a"?"800000000":"100000000",held_units:"0",available_units:account==="account-a"?"800000000":"100000000"},trial:{posted_units:"0",held_units:"0"},
          allowance:{limit_units:"125000000",remaining_units:"48250000",held_units:"0",generation:null,resets_at:"2027-01-08T00:00:00Z",effective_until:"2027-01-08T00:00:00Z"},subscription,
          capabilities:{subscriptions:true,auto_topup:true,card:true,crypto:true,admin:false}}; break;
        case "card_options": value={enabled:true,currency:"usd",min_amount_microusd:5000000,max_amount_microusd:1000000000}; break;
        case "plans": value={enabled:true,currency:"usd",billing_interval:"month",allowance_interval_days:7,plans:[{key:"starter",name:"Starter",amount_cents:250,weekly_units:"125000000",version:"v1"}]}; break;
        case "cards": value={cards:[{id:"pm_Fixture",brand:"visa",last4:"4242",exp_month:12,exp_year:2030}]}; break;
        case "auto_topup": value=auto; break;
        case "configure_auto_topup": auto={...auto,...action.settings,version:String(Number(auto.version)+1)}; value=auto; kind="auto_topup"; break;
        case "operations": case "card_payments": value=[]; break;
        case "usage": value={period:"all_time",scale:"1000000",total_units:"250000",allocation:{subscription_units:"250000",trial_units:"0"},purchased:{currency:"usd",scale:"100000000",total_units:"0",models:[]},models:[{provider:"near",model_id:"fixture",units:"250000"}]}; break;
        case "checkout": value={id:action.id,kind:"subscription",state:"pending",url:"https://checkout.stripe.com/c/pay/subscription",amount_cents:250,card:null,created_at:"2026-11-13T00:00:00Z"}; kind="operation"; break;
        case "create_card_payment":
          if (held) await new Promise<void>(resolve=>{gate=resolve;});
          value={id:action.id,amount_microusd:action.amount_microusd,credited_microusd:0,refunded_microusd:0,status:"open",checkout_url:"https://checkout.stripe.com/c/pay/fixture",expires_at:"2027-01-08T00:00:00Z",created_at:"2027-01-01T00:00:00Z",review_required:false,cancel_requested:false}; kind="card_payment"; break;
        default: throw new Error("Unexpected action: "+kind);
      }
      return {result:{kind,value:structuredClone(value)}};
    }}}});
  });
  await page.goto(origin+"tests/browser/credit-billing.fixture.html");
  try {await page.getByRole("button",{name:"Continue with card"}).waitFor();}
  catch (error) {
    throw new Error(JSON.stringify({errors,body:await page.locator("body").innerText(),state:await page.evaluate(() => ({visibility:document.visibilityState,mock:!!(window as any).__billingTest,calls:(window as any).__billingTest?.calls}))}),{cause:error});
  }
  await page.locator('details[aria-label="Automatic top-up"] > summary').click();
  await page.getByRole("button",{name:"Enable automatic top-up",exact:true}).waitFor();
  return {page,errors};
}
test("native credit controls separate balances and require explicit saved-card charge consent", async () => {
  const {page,errors}=await open();
  try {
    assert.match(await page.locator("body").innerText(),/Weekly credits.*48.*Purchased credit.*\$8\.00/s);
    assert.match(await page.locator("body").innerText(),/125 credits \/ week/);
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
    await page.getByRole("button",{name:"Subscribe",exact:true}).click();
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
    assert.match(await page.locator("body").innerText(),/Purchased credit\s+\$1\.00/);
    assert.doesNotMatch(await page.locator("body").innerText(),/Complete your payment in the browser/);
    const charges=await page.evaluate(()=>(window as any).__billingTest.calls.filter((c:any)=>c.action.kind==="create_card_payment"));
    assert.equal(charges.length,1); assert.equal(charges[0].account,"account-a");
    assert.deepEqual(errors,[]);
  } finally {await page.close();}
});

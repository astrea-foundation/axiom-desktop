import assert from "node:assert/strict";
import { before,after,test } from "node:test";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium,type Browser,type Page } from "playwright";
import { createServer,type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
let browser:Browser;let server:ViteDevServer;let origin:string;
const captures=process.env.AXIOM_REMOTE_MCP_CAPTURE_DIR;
before(async()=>{
  server=await createServer({configFile:false,root:fileURLToPath(new URL("../../src/renderer",import.meta.url)),plugins:[react(),tailwindcss()],server:{host:"127.0.0.1",port:0},logLevel:"error"});
  await server.listen();origin=server.resolvedUrls!.local[0]!;browser=await chromium.launch({headless:true});
  if(captures)await mkdir(captures,{recursive:true});
});
after(async()=>{await browser?.close();await server?.close();});
async function open(theme="light"){
  const page=await browser.newPage({viewport:{width:1120,height:860},bypassCSP:true});const errors:string[]=[];
  page.on("pageerror",error=>errors.push(error.message));page.setDefaultTimeout(8000);
  await page.addInitScript({content:"window.__name = fn => fn;"});
  await page.route("**/*",route=>{
    const url=route.request().url();
    if(url===origin)return route.fulfill({contentType:"text/html",body:`<!doctype html><html data-theme="${theme}"><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module">import RefreshRuntime from "/@react-refresh"; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type; window.__vite_plugin_react_preamble_installed__ = true; await import("/@fs/${fileURLToPath(new URL("./fixtures/remote-mcp.tsx",import.meta.url))}");</script></body></html>`});
    if(url.startsWith(origin))return route.continue();return route.abort();
  });
  await page.goto(origin);await page.getByRole("button",{name:"Agent",exact:true}).waitFor();await page.evaluate(()=>document.fonts.ready);
  return {page,errors};
}
async function capture(page:Page,name:string){if(captures)await page.screenshot({path:`${captures}/${name}.png`,animations:"disabled"});}
for(const theme of ["light","dark"])test(`remote tools expose destinations and remain independent of Agent (${theme})`,async()=>{
  const {page,errors}=await open(theme);try{
    const agent=page.getByRole("button",{name:"Agent",exact:true});
    assert.equal(await page.getByRole("button",{name:"Tools",exact:true}).count(),0);
    assert.match(await agent.getAttribute("aria-description")??"",/including remote tools/);
    assert.equal(await page.getByRole("button",{name:"Agent",exact:true}).getAttribute("aria-pressed"),"false");
    await agent.click();const dialog=page.getByRole("dialog",{name:"Agent settings"});
    await dialog.getByRole("heading",{name:"Remote services",exact:true}).waitFor();
    await dialog.getByRole("heading",{name:"On this device",exact:true}).waitFor();
    assert.match(await dialog.innerText(),/outside Axiom’s attested environment/);
    assert.match(await dialog.innerText(),/mcp.example.com/);
    await dialog.getByRole("checkbox",{name:"Use Project tracker in this conversation"}).check();
    assert.equal(await dialog.getByRole("button",{name:"Save changes",exact:true}).count(),0);
    assert.equal(await dialog.getByRole("button",{name:"Close Agent settings",exact:true}).evaluate(element=>{
      const bounds=element.getBoundingClientRect();return bounds.top>=0 && bounds.bottom<=innerHeight;
    }),true,"Close stays visible while the connections list scrolls");
    await dialog.getByRole("heading",{name:"MCP connections",exact:true}).scrollIntoViewIfNeeded();
    await capture(page,`${theme}-agent-connections`);
    await dialog.getByRole("button",{name:"Close Agent settings",exact:true}).click();await dialog.waitFor({state:"hidden"});
    assert.equal((await page.evaluate(()=>(window as any).__remoteMcpTest.selection())).mcpTools.length,2);
    assert.equal(await page.getByRole("button",{name:"Agent",exact:true}).getAttribute("aria-pressed"),"false");
    await page.getByRole("button",{name:/View privacy proof$/}).click();const proof=page.getByRole("dialog",{name:"Privacy proof",exact:true});
    const external=proof.getByRole("region",{name:"External services"});
    assert.match(await external.innerText(),/Enabled for this conversation: mcp.example.com/);
    assert.match(await external.innerText(),/Requests attempted in loaded history: history.example.com/);
    assert.match(await proof.innerText(),/Model inference and environment attestation/);
    await external.scrollIntoViewIfNeeded();await capture(page,`${theme}-privacy`);
    await page.keyboard.press("Escape");
    await page.getByRole("button",{name:/Called Project tracker.*show details/}).click();
    assert.match(await page.locator('[data-tool-call-id="search"]').innerText(),/Destination: history.example.com/);
    assert.match(await page.locator('[data-tool-call-id="search"]').innerText(),/Inputs sent to this service/);
    await capture(page,`${theme}-activity`);assert.deepEqual(errors,[]);
  }finally{await page.close();}
});
test("remote connection form discloses destination before connecting and never displays saved tokens",async()=>{
  const {page,errors}=await open();try{
    await page.evaluate(()=>(window as any).__remoteMcpTest.setView("settings"));
    await page.getByRole("button",{name:"Add connection",exact:true}).click();
    await page.getByRole("textbox",{name:/^Name/}).fill("Support tools");
    await page.getByRole("textbox",{name:"Service URL",exact:true}).fill("https://support.example.com/mcp");
    await page.getByRole("combobox",{name:"Authentication",exact:true}).selectOption("bearer");
    await page.getByLabel("API token",{exact:true}).fill("synthetic-test-bearer-value");
    const notice=page.locator("[data-remote-mcp-notice]").first();assert.match(await notice.innerText(),/support.example.com/);
    await capture(page,"remote-connection-form");
    await page.getByRole("button",{name:"Connect",exact:true}).click();
    const card=page.getByRole("region",{name:"Support tools connection"});await card.getByText("Ready with 1 tool",{exact:true}).waitFor();
    assert.doesNotMatch(await page.locator("body").innerText(),/synthetic-test-bearer-value/);
    assert.doesNotMatch(JSON.stringify(await page.evaluate(()=>(window as any).__remoteMcpTest.snapshot())),/synthetic-test-bearer-value/);
    await card.getByRole("button",{name:"Edit",exact:true}).click();assert.equal(await page.getByLabel("API token",{exact:true}).inputValue(),"");
    await page.getByRole("textbox",{name:"Name",exact:true}).fill("Customer support");
    await page.getByRole("button",{name:"Connect",exact:true}).click();
    const renamed=page.getByRole("region",{name:"Customer support connection"});
    await renamed.getByText("Ready with 1 tool",{exact:true}).waitFor();
    assert.equal((await page.evaluate(()=>(window as any).__remoteMcpTest.snapshot())).servers.find((server:any)=>server.displayName==="Customer support").name,"support_tools");
    await renamed.getByRole("button",{name:"Disconnect",exact:true}).click();await renamed.getByText("Needs sign-in or API token",{exact:true}).waitFor();
    assert.deepEqual(errors,[]);
  }finally{await page.close();}
});
test("failed remote calls preserve inputs and distinguish attempts from calls that never started",async()=>{
  const {page,errors}=await open();try{
    await page.evaluate(()=>(window as any).__remoteMcpTest.setTool({status:"failed",started:true,content:[{type:"text",text:"Outcome unknown; call was not retried"}]}));
    await page.getByRole("button",{name:/Failed to call Project tracker.*show details/}).click();
    const card=page.locator('[data-tool-call-id="search"]');assert.match(await card.innerText(),/Request attempted/);assert.match(await card.innerText(),/customer onboarding/);
    await page.evaluate(()=>(window as any).__remoteMcpTest.setTool({started:false}));
    assert.match(await card.innerText(),/This call did not start; inputs were not sent by this call/);
    assert.deepEqual(errors,[]);
  }finally{await page.close();}
});

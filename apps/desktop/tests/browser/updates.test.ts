import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium, type Browser, type Page } from 'playwright';
import { createServer, type ViteDevServer } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { desktopReleaseFixture } from '../../../../fixtures/desktop-releases.mts';
import type { UpdateState } from '../../src/shared/updates';

let browser: Browser, server: ViteDevServer, origin: string;
const captures = process.env.AXIOM_UPDATE_CAPTURE_DIR;
const release = desktopReleaseFixture();
before(async () => {
  server = await createServer({ configFile:false, root:fileURLToPath(new URL('../../src/renderer',import.meta.url)), plugins:[react(),tailwindcss()],
    server:{host:'127.0.0.1',port:0},logLevel:'error' });
  await server.listen(); origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({headless:true});
  if (captures) await mkdir(captures,{recursive:true});
});
after(async () => { await browser?.close(); await server?.close(); });

async function open(theme = 'light', overrides: Partial<UpdateState> = {}) {
  const page = await browser.newPage({viewport:{width:1120,height:800},bypassCSP:true});
  page.setDefaultTimeout(8000);
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort());
  await page.addInitScript({content:'window.__name = (fn) => fn;'});
  await page.addInitScript(({theme, initial}) => {
    localStorage.setItem('axiom.theme',theme);
    let state = initial;
    const listeners = new Set<(state: UpdateState) => void>();
    let installs = 0, cancellations = 0;
    let checks = 0;
    let finish: ((state: UpdateState) => void) | null = null;
    const update = (patch: Partial<UpdateState>) => { state = {...state,...patch,revision:state.revision + 1}; listeners.forEach(listener => listener(structuredClone(state))); };
    Object.assign(window, {
      __updatesTest: {
        update, installs: () => installs, cancellations: () => cancellations, checks: () => checks,
        finish: (failed = false) => {
          update(failed ? {status:'error',error:'Couldn’t download and verify the update. Try again.',download:null}
            : {status:'waiting',download:{...state.download!,received:state.download!.total}});
          finish?.(structuredClone(state)); finish = null;
        },
      },
      axiomDesktop: {
        platform:'linux', setTheme:() => {},
        isFullScreen:async () => false,onFullScreenChange:() => () => {},
        isMaximized:async () => false,onMaximizedChange:() => () => {},
        isFocused:async () => true,onActiveChange:() => () => {},
        agent: {
          getState: async () => ({ connected:false, runtimeInstanceId:null, sessions:{},activeSessionId:null,
            catalog:[],collections:{revision:0,collections:[]},preferences:null,account:null,billing:null,diagnostic:'',error:null }),
          onState:() => () => {},
        },
        updates: {
          getState:async () => structuredClone(state),
          onStateChange:(listener: (state: UpdateState) => void) => { listeners.add(listener); return () => listeners.delete(listener); },
          check:async () => { checks++; update({status:'current',error:null,checkedAt:Date.now()}); return structuredClone(state); },
          onBeforeRestart:() => () => {},
          install:() => {
            installs++;
            const file = state.release!.downloads.find(file => file.platform === state.platform && file.arch === state.arch && file.format === state.format)!;
            update({status:'downloading',error:null,download:{name:file.name,received:0,total:file.bytes}});
            return new Promise<UpdateState>(resolve => { finish = resolve; });
          },
          cancel:async () => { cancellations++; update({status:'available',download:null}); finish?.(structuredClone(state)); finish=null; return structuredClone(state); },
        },
      },
    });
  }, {theme, initial:{revision:1,status:'available',currentVersion:'0.1.0',platform:'linux',arch:'x64',format:'AppImage',packaged:true,release,checkedAt:Date.now(),error:null,download:null,...overrides} as UpdateState});
  await page.goto(origin);
  try { await page.getByRole('button',{name:/View updates$/}).waitFor(); }
  catch (error) { console.error(errors,await page.locator('body').innerText()); await page.close(); throw error; }
  return {page,errors};
}
const settings = (page: Page) => page.getByRole('region',{name:'Settings',exact:true});
const capture = async (page: Page, name: string) => {
  if (!captures) return;
  await page.evaluate(() => Promise.all(document.getAnimations().filter(animation => Number.isFinite(animation.effect?.getComputedTiming().iterations)).map(animation => animation.finished.catch(() => {}))));
  await page.screenshot({path:`${captures}/${name}.png`});
};

for (const theme of ['light','dark']) test(`Update and restart preserves visible progress and supports waiting/cancellation (${theme})`, async () => {
  const {page,errors} = await open(theme);
  try {
    const notice = page.getByRole('button',{name:/View updates$/}); await notice.click();
    assert.equal(await settings(page).getByRole('tab',{name:'Updates',exact:true}).getAttribute('aria-selected'),'true');
    assert.match(await settings(page).innerText(),/Installed version: 0.1.0/);
    await capture(page,`${theme}-available`);
    await page.getByRole('button',{name:'Update and restart',exact:true}).click();
    await page.getByRole('progressbar',{name:'Update download'}).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__updatesTest.installs()),1);
    await page.evaluate(() => (window as any).__updatesTest.update({download:{name:'Axiom.AppImage',received:50,total:100}}));
    await page.waitForFunction(() => document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow') === '50');
    await page.keyboard.press('Escape'); await settings(page).waitFor({state:'detached'});
    await notice.click(); assert.equal(await page.getByRole('progressbar').getAttribute('aria-valuenow'),'50');
    await capture(page,`${theme}-downloading`);
    await page.evaluate(() => (window as any).__updatesTest.finish());
    await page.getByText('Ready to restart',{exact:true}).waitFor();
    assert.match(await settings(page).innerText(),/Finish active chats and stop the local proxy/);
    await page.getByRole('button',{name:'Cancel',exact:true}).click();
    await page.getByRole('button',{name:'Update and restart',exact:true}).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__updatesTest.cancellations()),1);
    assert.deepEqual(errors,[]);
  } finally {await page.close();}
});
test('failure remains visible and offers a retry, while missing installers cannot be applied',async()=>{
  const {page,errors}=await open('light',{status:'error',error:'Update checksum mismatch'});
  try {
    await page.getByRole('button',{name:/View updates$/}).click();
    assert.equal(await page.getByRole('alert').innerText(),'Update checksum mismatch');
    assert.equal(await page.getByRole('button',{name:'Update and restart',exact:true}).isEnabled(),true);
    await page.evaluate(()=>(window as any).__updatesTest.update({release:{...(window as any).__unused,version:'0.2.0',downloads:[]}}));
    await page.getByText('A matching installer has not been published yet.').waitFor();
    assert.equal(await page.getByRole('button',{name:'Update and restart',exact:true}).isDisabled(),true);
    assert.deepEqual(errors,[]);
  }finally{await page.close();}
});

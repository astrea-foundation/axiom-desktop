import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium, type Browser, type Page } from 'playwright';
import { createServer, type ViteDevServer } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { desktopReleaseFixture } from '../../../../fixtures/desktop-releases.mts';
import type { PackageFormat, UpdateState } from '../../src/shared/updates';

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
    const downloads: (PackageFormat | undefined)[] = [];
    let revealed = 0, checks = 0;
    let finish: ((state: UpdateState) => void) | null = null;
    const update = (patch: Partial<UpdateState>) => { state = {...state,...patch,revision:state.revision + 1}; listeners.forEach(listener => listener(structuredClone(state))); };
    Object.assign(window, {
      __updatesTest: {
        update, downloads: () => downloads, revealed: () => revealed, checks: () => checks,
        finish: (failed = false) => {
          update(failed ? {status:'error',error:'Couldn’t download and verify the update. Try again.',download:null}
            : {status:'downloaded',download:{...state.download!,received:state.download!.total}});
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
          download:(format?: PackageFormat) => {
            downloads.push(format);
            const file = state.release!.downloads.find(file => file.platform === state.platform && file.arch === state.arch && file.format === format)!;
            update({status:'downloading',error:null,download:{name:file.name,received:0,total:file.bytes}});
            return new Promise<UpdateState>(resolve => { finish = resolve; });
          },
          reveal:async () => { revealed++; },
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

for (const theme of ['light','dark']) test(`update notice opens integrated Settings and downloads the matching package (${theme})`, async () => {
  const {page,errors} = await open(theme);
  try {
    await capture(page,`${theme}-notice`);
    const notice = page.getByRole('button',{name:/View updates$/});
    await notice.click();
    await page.waitForFunction(() => document.activeElement?.getAttribute('data-category') === 'updates');
    assert.equal(await settings(page).getByRole('tab',{name:'Updates',exact:true}).getAttribute('aria-selected'),'true');
    assert.match(await settings(page).innerText(),/Installed · Axiom 0.1.0 · Linux · x64 · AppImage/);
    assert.equal(await page.getByRole('combobox').count(),0);
    assert.equal(await page.getByText('SHA-256',{exact:true}).isVisible(),false);
    await capture(page,`${theme}-available`);
    await page.getByRole('button',{name:'Download update',exact:true}).click();
    await page.getByRole('progressbar',{name:'Update download'}).waitFor();
    assert.deepEqual(await page.evaluate(() => (window as any).__updatesTest.downloads()),['AppImage']);
    await page.evaluate(() => (window as any).__updatesTest.update({download:{name:'Axiom.AppImage',received:50,total:100}}));
    await page.waitForFunction(() => document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow') === '50');
    assert.equal(await page.getByRole('progressbar').getAttribute('aria-valuenow'),'50');
    await capture(page,`${theme}-downloading`);
    await page.keyboard.press('Escape'); await settings(page).waitFor({state:'detached'});
    await notice.click();
    assert.equal(await page.getByRole('progressbar').getAttribute('aria-valuenow'),'50','download survives closing Settings');
    await page.evaluate(() => (window as any).__updatesTest.finish());
    await page.getByText('Your update is ready',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Show in folder'}).click();
    assert.equal(await page.evaluate(() => (window as any).__updatesTest.revealed()),1);
    await page.locator('summary').filter({hasText:'Download details'}).click();
    await page.getByText('File size and SHA-256 matched the release manifest.',{exact:true}).waitFor({state:'visible'});
    assert.match(await settings(page).innerText(),/File size and SHA-256 matched/);
    await capture(page,`${theme}-downloaded`);
    await page.keyboard.press('Escape'); await settings(page).waitFor({state:'detached'});
    assert.equal(await notice.evaluate(element => element === document.activeElement),true);
    assert.deepEqual(errors,[]);
  } finally { await page.close(); }
});

test('older installations choose a format without changing platform or architecture', async () => {
  const {page,errors} = await open('light',{platform:'mac',arch:'arm64',format:null});
  try {
    await page.getByRole('button',{name:/View updates$/}).click();
    assert.equal(await page.getByRole('button',{name:'Download update',exact:true}).isDisabled(),true);
    const choices = await page.getByRole('combobox',{name:'Installer format'}).locator('option').allTextContents();
    assert.equal(choices.length,4); assert.ok(choices.some(label => label.startsWith('ZIP')));
    assert.ok(choices.every(label => !label.includes('EXE') && !label.includes('AppImage')));
    await page.getByRole('combobox').selectOption('zip');
    await page.getByRole('button',{name:'Download update',exact:true}).click();
    assert.deepEqual(await page.evaluate(() => (window as any).__updatesTest.downloads()),['zip']);
    await page.evaluate(() => (window as any).__updatesTest.finish(true));
    await page.getByRole('alert').waitFor();
    assert.equal(await page.getByRole('button',{name:'Show in folder'}).count(),0);
    assert.equal(await page.getByRole('button',{name:'Download update',exact:true}).isEnabled(),true);
    assert.deepEqual(errors,[]);
  } finally { await page.close(); }
});

test('missing matching build is informative without an automatic fallback; manual check stays accessible', async () => {
  const noMatch = {...release,downloads:release.downloads.filter(file => file.format !== 'AppImage')};
  const {page,errors} = await open('dark',{release:noMatch});
  try {
    await page.getByRole('button',{name:/View updates$/}).click();
    assert.match(await settings(page).innerText(),/matching build hasn’t been published/);
    assert.equal(await page.getByRole('button',{name:'Download update',exact:true}).isDisabled(),true);
    await page.getByRole('button',{name:'Check for updates',exact:true}).click();
    await page.getByText('You’re up to date',{exact:true}).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__updatesTest.checks()),1);
    await page.evaluate(() => (window as any).__updatesTest.update({status:'error',release:null,error:'Couldn’t check for updates. Try again when you’re online.'}));
    await page.getByRole('alert').waitFor();
    assert.equal(await page.getByRole('button',{name:'Download update',exact:true}).count(),0);
    assert.deepEqual(errors,[]);
  } finally { await page.close(); }
});

test('update screen and technical details fit narrow windows', async () => {
  const {page,errors} = await open('dark');
  try {
    await page.setViewportSize({width:540,height:600});
    await page.getByRole('button',{name:/View updates$/}).click();
    await page.locator('summary').filter({hasText:'Download details'}).click();
    const panel = settings(page).getByRole('tabpanel');
    assert.equal(await panel.evaluate(element => element.scrollWidth <= element.clientWidth),true);
    await capture(page,'narrow-dark');
    assert.deepEqual(errors,[]);
  } finally { await page.close(); }
});

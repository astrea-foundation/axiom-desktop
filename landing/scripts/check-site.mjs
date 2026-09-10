import assert from 'node:assert/strict';
import {mkdir, readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {chromium} from '../../apps/auth/node_modules/playwright/index.mjs';

const origin=process.env.SITE_ORIGIN ?? 'http://127.0.0.1:4177';
const directory=fileURLToPath(new URL('../../dist/website-deploy/',import.meta.url));
await mkdir(directory,{recursive:true});
const browser=await chromium.launch({args:['--enable-unsafe-swiftshader']});
const failures=[];
try {
  for (const [name,viewport] of [['desktop',{width:1440,height:1000}],['mobile',{width:390,height:844}]]) {
    const page=await browser.newPage({viewport,colorScheme:'light'});
    page.on('pageerror',error=>failures.push(`${name}: ${error.message}`));
    page.on('response',response=>{if(response.status()>=400 && response.url().startsWith(origin)) failures.push(`${name}: HTTP ${response.status()} ${response.url()}`);});
    for(const path of ['/','/manifesto','/downloads']) {
      const response=await page.goto(origin+path,{waitUntil:'networkidle',timeout:45000});
      assert.equal(response.status(),200);await page.evaluate(()=>document.fonts.ready);await page.waitForTimeout(1200);
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,`${name} ${path}: horizontal overflow`);
      assert.equal(await page.locator('a[href="#"]').count(),0,`${path}: placeholder link`);
      await page.screenshot({path:`${directory}/${name}-${path==='/'?'home':path.slice(1)}.png`,fullPage:path==='/downloads'});
      if(path==='/') {
        const links=await page.locator('a').evaluateAll(els=>els.filter(a=>a.querySelector('[data-dl-label]')).map(a=>a.getAttribute('href')));
        assert(links.length>=3 && links.every(h=>h.startsWith('/downloads')),`Download CTAs must reach the release selector: ${links}`);
        assert.equal(await page.getByRole('link',{name:'Sign in',exact:true}).getAttribute('href'),'https://auth.axiom.stream/signin');
      }
      if(path==='/downloads') {
        assert.equal(await page.locator('.platforms .download-link').count(),11);
        await page.getByText('DMG and ZIP downloads',{exact:true}).click();
        await page.getByText('Verify your download · SHA-256 checksums',{exact:true}).click();
        assert.equal(await page.locator('.checksums li').count(),11);
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
        await page.locator('#versions > summary').click();
        await page.locator('#versions details > summary').filter({hasText:'Version 0.1.0'}).click();
        assert.equal(await page.locator('#versions a.download-link:visible').count(),11);
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
        await page.screenshot({path:`${directory}/${name}-release-history.png`,fullPage:true});
      }
    }
    await page.close();console.log(`PASS ${name}: website, manifesto, downloads, links and layout`);
  }
} finally {await browser.close();}
assert.deepEqual(failures,[]);
for(const path of ['/signin','/account','/privacy','/terms']) {
  const r=await fetch(origin+path,{redirect:'manual'});assert.equal(r.status,302);assert.equal(r.headers.get('location'),'https://auth.axiom.stream'+path);
}
assert.equal((await fetch(origin+'/not-a-real-page')).status,404);
assert.equal((await fetch(origin+'/api/not-a-route')).status,404);
const manifest=JSON.parse(await readFile(new URL('../releases.json',import.meta.url),'utf8'));
const latest=await (await fetch(origin+'/api/releases/latest')).json();
assert.equal(latest.version,manifest.version);
const catalog=await (await fetch(origin+'/api/releases')).json();
assert.equal(catalog.latest,manifest.version);
const downloads=catalog.releases.flatMap(release=>release.downloads);
for(const file of downloads) {
  const response=await fetch(file.url,{method:'HEAD',signal:AbortSignal.timeout(30000)});
  assert.equal(response.status,200,file.name);assert.equal(Number(response.headers.get('content-length')),file.bytes,file.name);
}
console.log(`PASS release API, auth/legal redirects, 404s, and all ${downloads.length} published installer sizes`);

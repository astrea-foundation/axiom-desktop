import test from 'node:test';
import assert from 'node:assert/strict';
import worker from '../worker.mjs';
import {readFile} from 'node:fs/promises';
import {parseRelease} from '../../packages/desktop-releases/manifest.mjs';

const requests = [];
const env = {ASSETS:{fetch:async request => {
  const path = new URL(request.url).pathname; requests.push(path);
  if (path.startsWith('/releases/')) {
    try { return new Response(await readFile(new URL(`../.site${path}`, import.meta.url)), {headers:{'content-type':'application/json'}}); }
    catch { return new Response('Not Found', {status:404}); }
  }
  if (path === '/index.html' || path === '/downloads.html' || path === '/404.html') return new Response('<h1>Axiom</h1>', {headers:{'content-type':'text/html'}});
  if (path === '/dist/landing-ABCDEFGH.js') return new Response('export {}', {headers:{'content-type':'text/javascript'}});
  return new Response('Not Found', {status:404});
}}};

test('new visitors receive the website directly, without a welcome cookie', async () => {
  const response = await worker.fetch(new Request('https://axiom.stream/'),env);
  assert.equal(response.status,200);assert.equal(response.headers.get('set-cookie'),null);
  assert.equal(response.headers.get('cache-control'),'no-cache');assert.match(await response.text(),/Axiom/);
});

test('release API exposes latest and every archived version with matching artifacts', async () => {
  const latest = await worker.fetch(new Request('https://axiom.stream/api/releases/latest'),env);
  assert.equal(latest.status,200);
  assert.equal(latest.headers.get('content-type'),'application/json; charset=utf-8');
  assert.equal(latest.headers.get('cache-control'),'public, max-age=60, must-revalidate');
  assert.equal(latest.headers.get('access-control-allow-origin'),'*');
  assert.equal(latest.headers.get('set-cookie'),null);
  const release = parseRelease(await latest.json());
  const catalog = await (await worker.fetch(new Request('https://axiom.stream/api/releases'),env)).json();
  assert.equal(catalog.schemaVersion,1); assert.equal(catalog.latest,release.version);
  assert.ok(catalog.releases.some(item => item.version === '0.1.0'));
  for (const item of catalog.releases) {
    const version = await (await worker.fetch(new Request(`https://axiom.stream/api/releases/${item.version}?ignored=true`),env)).json();
    assert.deepEqual(parseRelease(version), item);
    assert.ok(item.downloads.every(file => file.url.includes(`/downloads/${item.version}/`)));
  }
  const html = await readFile(new URL('../.site/downloads.html', import.meta.url),'utf8');
  assert.ok(html.includes(`VERSION ${release.version}`));
  for (const item of catalog.releases) for (const file of item.downloads) assert.ok(html.includes(file.url));
});

test('release API handles HEAD, missing versions, malformed selectors, and methods without a proxy', async () => {
  const head = await worker.fetch(new Request('https://axiom.stream/api/releases/latest',{method:'HEAD'}),env);
  assert.equal(head.status,200); assert.equal(await head.text(),'');
  const missing = await worker.fetch(new Request('https://axiom.stream/api/releases/999.0.0'),env);
  assert.equal(missing.status,404); assert.deepEqual(await missing.json(),{error:'Release not found'});
  for (const suffix of ['/latest/extra','/01.0.0','/https://evil.invalid','/0.1.0-beta']) {
    assert.equal((await worker.fetch(new Request(`https://axiom.stream/api/releases${suffix}`),env)).status,404);
  }
  assert.equal((await worker.fetch(new Request('https://axiom.stream/api/releases/latest',{method:'POST'}),env)).status,405);
});
test('old welcome links redirect to the canonical website', async () => {
  const response = await worker.fetch(new Request('https://welcome.axiom.stream/downloads?source=old'),env);
  assert.equal(response.headers.get('location'),'https://axiom.stream/downloads?source=old');
});
test('account redirects preserve native authorization state on a fixed origin', async () => {
  const path='/native/authorize?authorization_id=test&state=test-state&return_to=https://evil.invalid';
  const response=await worker.fetch(new Request('https://axiom.stream'+path),env);
  assert.equal(response.headers.get('location'),'https://auth.axiom.stream'+path);
});
test('marketing never proxies API calls, stores cookies, or accepts POSTs', async () => {
  const before=requests.length;
  assert.equal((await worker.fetch(new Request('https://axiom.stream/api/v1/runs'),env)).status,404);
  assert.equal((await worker.fetch(new Request('https://axiom.stream/',{method:'POST',body:'test'}),env)).status,405);
  assert.equal(requests.length,before);
});
test('downloads and missing documents have correct status, HEAD and caching', async () => {
  assert.equal((await worker.fetch(new Request('https://axiom.stream/downloads'),env)).status,200);
  assert.equal((await worker.fetch(new Request('https://axiom.stream/missing'),env)).status,404);
  assert.equal(await (await worker.fetch(new Request('https://axiom.stream/',{method:'HEAD'}),env)).text(),'');
  const js=await worker.fetch(new Request('https://axiom.stream/dist/landing-ABCDEFGH.js'),env);
  assert.equal(js.headers.get('cache-control'),'public, max-age=31536000, immutable');
});

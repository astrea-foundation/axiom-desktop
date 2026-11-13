import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { compareVersions, FORMATS, parseRelease, RELEASE_API, type Release } from '../../../packages/desktop-releases/manifest.mjs';
import { updateIdentity } from '../src/main/update-identity';
import { readUpdateJson, saveUpdate } from '../src/main/update-download';
import { UpdateService } from '../src/main/update-service';
import type { UpdateIdentity, UpdateState } from '../src/shared/updates';

const contents = Buffer.from('an installer fixture');
function release(version = '0.2.0'): Release {
  return parseRelease({ version, signing: 'unsigned', downloads: Object.entries(FORMATS).flatMap(([platform, formats]) =>
    ['x64', 'arm64'].flatMap(arch => formats.map(format => {
      const name = `Axiom-${version}-${platform}-${arch}.${format === 'pacman' ? 'pkg.tar.zst' : format}`;
      return { name, platform, arch, format, bytes: contents.length, sha256: createHash('sha256').update(contents).digest('hex'), url: `https://cdn.axiom.stream/downloads/${version}/${name}` };
    }))) });
}
const identity: UpdateIdentity = { currentVersion: '0.1.9', packaged: true, platform: 'linux', arch: 'x64', format: 'AppImage' };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return {promise, resolve}; }
function setup(current = identity, remote: unknown = release()) {
  const events: UpdateState[] = [], chosen: string[] = [], saved: string[] = [], revealed: string[] = [], requests: {url: string; options: RequestInit | undefined}[] = [];
  const dependencies = {
    fetch: (async (url, options) => { requests.push({url: String(url), options}); return Response.json(remote); }) as typeof fetch,
    chooseDestination: async (file: {name: string}) => { chosen.push(file.name); return `/downloads/${file.name}`; },
    save: (async file => { saved.push(file.name); }) as typeof saveUpdate,
    emit: (state: UpdateState) => { events.push(state); }, reveal: (path: string) => { revealed.push(path); },
  };
  return { service: new UpdateService(current, dependencies), dependencies, events, chosen, saved, revealed, requests };
}

test('version comparison is numeric and refuses prereleases and ambiguous versions', () => {
  assert.equal(compareVersions('0.10.0', '0.9.9'), 1);
  assert.equal(compareVersions('1.0.0', '0.99.99'), 1);
  assert.equal(compareVersions('0.1.0', '0.1.0'), 0);
  for (const version of ['01.2.3', '1.0.0-beta', '1.0.0+test', '1.2', '-1.2.3']) assert.throws(() => compareVersions(version, '1.0.0'));
});

test('legacy metadata gains explicit formats, and invalid downloads fail closed', () => {
  const legacy = release();
  for (const file of legacy.downloads) delete (file as any).format;
  assert.equal(parseRelease(legacy).downloads[0]!.format, 'pkg');
  for (const patch of [
    {url: 'https://evil.invalid/update.exe'}, {url: release().downloads[0]!.url + '?redirect=elsewhere'},
    {url: release().downloads[0]!.url.replace('https:', 'http:')}, {name: '../installer.pkg'}, {sha256: 'invalid'},
    {bytes: 0}, {bytes: 3 * 1024 ** 3}, {arch: 'x64', name: 'Axiom-0.2.0-mac-arm64.pkg'}, {format: 'dmg'}, {platform: 'linux'},
  ]) {
    const candidate = release(); Object.assign(candidate.downloads[0]!, patch);
    assert.throws(() => parseRelease(candidate), JSON.stringify(patch));
  }
  const duplicate = release(); duplicate.downloads.push(duplicate.downloads[0]!);
  assert.throws(() => parseRelease(duplicate));
  assert.throws(() => parseRelease({...release(), schemaVersion: 2}));
});

test('identity uses the running architecture, stamped format, and conservative legacy detection', () => {
  for (const [platform, formats] of Object.entries(FORMATS)) for (const format of formats) {
    const result = updateIdentity({ version: '0.1.0', platform: {mac:'darwin',win:'win32',linux:'linux'}[platform]!, arch: 'arm64', packaged: true, packageFormat: format });
    assert.equal(result.format, format); assert.equal(result.arch, 'arm64');
  }
  assert.equal(updateIdentity({version:'0.1.0',platform:'darwin',arch:'x64',packaged:true}).format, null);
  assert.equal(updateIdentity({version:'0.1.0',platform:'linux',arch:'x64',packaged:true,appImage:'/app.AppImage'}).format, 'AppImage');
  assert.equal(updateIdentity({version:'0.1.0',platform:'linux',arch:'x64',packaged:true,packageFormat:'exe'}).format, null);
  assert.equal(updateIdentity({version:'0.1.0',platform:'win32',arch:'arm64',packaged:true}).format, 'exe');
});

test('every platform, architecture, and installer downloads exactly its matching new artifact', async () => {
  for (const artifact of release().downloads) {
    const {service, chosen, saved, revealed} = setup({...identity, platform:artifact.platform, arch:artifact.arch, format:artifact.format});
    assert.equal((await service.check()).status, 'available');
    assert.equal((await service.download()).status, 'downloaded');
    assert.deepEqual(chosen, [artifact.name]); assert.deepEqual(saved, [artifact.name]);
    service.reveal(); assert.deepEqual(revealed, [`/downloads/${artifact.name}`]);
    await service.dispose();
  }
});

test('unsupported variants never fall back; legacy users can choose only matching formats', async () => {
  const missing = release(); missing.downloads = missing.downloads.filter(file => file.format !== 'AppImage');
  const a = setup(identity, missing); await a.service.check();
  await assert.rejects(a.service.download(), /matching installer/); assert.deepEqual(a.saved, []);
  await assert.rejects(a.service.download('deb'), /matching installer/); assert.deepEqual(a.saved, []);
  const b = setup({...identity, platform:'mac', arch:'arm64', format:null}); await b.service.check();
  await assert.rejects(b.service.download(), /matching installer/);
  await assert.rejects(b.service.download('exe'), /matching installer/);
  await b.service.download('zip'); assert.deepEqual(b.saved, ['Axiom-0.2.0-mac-arm64.zip']);
});

test('same and older releases cannot trigger a download; development makes no requests', async () => {
  for (const version of ['0.1.9', '0.1.0']) {
    const a = setup(identity, release(version)); assert.equal((await a.service.check()).status, 'current');
    await a.service.download(); assert.deepEqual(a.saved, []);
  }
  const dev = setup({...identity, packaged:false}); dev.service.start();
  assert.equal((await dev.service.check()).status, 'disabled'); assert.deepEqual(dev.requests, []); await dev.service.dispose();
});

test('checks use the fixed anonymous endpoint, coalesce, and fail without a stale success', async () => {
  const a = setup(); const pending = deferred<Response>();
  a.dependencies.fetch = (async () => pending.promise) as typeof fetch;
  const first = a.service.check(); assert.equal(a.service.check(), first);
  pending.resolve(Response.json(release())); assert.equal((await first).status, 'available');
  a.dependencies.fetch = (async () => { throw new Error('offline'); }) as typeof fetch;
  assert.equal((await a.service.check()).status, 'error'); assert.equal(a.service.snapshot().release, null);
  await a.service.download(); assert.deepEqual(a.saved, []);
  const b = setup(); await b.service.check();
  assert.equal(b.requests[0]!.url, RELEASE_API);
  assert.equal(b.requests[0]!.options?.credentials, 'omit'); assert.equal(b.requests[0]!.options?.redirect, 'error');
  const headers = new Headers(b.requests[0]!.options?.headers);
  assert.deepEqual([...headers.keys()], ['accept']);
});

test('download clicks coalesce across save dialog and transfer; cancellation and failure can retry', async () => {
  const a = setup(); await a.service.check();
  const destination = deferred<string | null>();
  a.dependencies.chooseDestination = async () => destination.promise as Promise<string>;
  const first = a.service.download(); assert.equal(a.service.download(), first);
  assert.equal((await a.service.check()).status, 'downloading');
  destination.resolve(null); assert.equal((await first).status, 'available'); assert.deepEqual(a.saved, []);
  a.dependencies.chooseDestination = async () => '/downloads/file';
  a.dependencies.save = async () => { throw new Error('corrupt file'); };
  assert.equal((await a.service.download()).status, 'error'); a.service.reveal(); assert.deepEqual(a.revealed, []);
  a.dependencies.save = async () => {};
  assert.equal((await a.service.download()).status, 'downloaded');
  assert.equal((await a.service.check()).status, 'downloaded');
});

test('release response size is bounded and invalid JSON is rejected', async () => {
  await assert.rejects(readUpdateJson(new Response('x'.repeat(100)), 99));
  await assert.rejects(readUpdateJson(new Response('not json')));
  await assert.rejects(readUpdateJson(new Response('{}', {status:503})));
});

test('a downloaded installer survives failed checks and does not hide a newer release', async () => {
  const a = setup(); await a.service.check(); await a.service.download();
  a.dependencies.fetch = (async () => { throw new Error('offline'); }) as typeof fetch;
  assert.equal((await a.service.check()).status,'downloaded');
  a.service.reveal(); assert.equal(a.revealed.length,1);
  a.dependencies.fetch = (async () => Response.json(release('0.3.0'))) as typeof fetch;
  const state = await a.service.check(); assert.equal(state.status,'available'); assert.equal(state.release?.version,'0.3.0');
  a.service.reveal(); assert.equal(a.revealed.length,1);
});

test('verified download is saved atomically; corruption and truncation preserve existing files and clean partials', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'axiom-updates-'));
  const file = release().downloads[0]!;
  const destination = join(directory, file.name);
  try {
    for (const body of [Buffer.from('bad bytes of same len'), contents.subarray(0, 3), Buffer.concat([contents, contents])]) {
      await writeFile(destination, 'existing download');
      const fetcher = (async () => new Response(body)) as typeof fetch;
      await assert.rejects(saveUpdate(file, destination, new AbortController().signal, () => {}, fetcher));
      assert.equal(await readFile(destination, 'utf8'), 'existing download');
      assert.deepEqual(await readdir(directory), [file.name]);
    }
    const progress: number[] = [];
    await saveUpdate(file, destination, new AbortController().signal, count => progress.push(count), (async () => new Response(contents)) as typeof fetch);
    assert.deepEqual(await readFile(destination), contents); assert.equal(progress.at(-1), contents.length);
    const abort = new AbortController(); abort.abort();
    await assert.rejects(saveUpdate(file, destination, abort.signal, () => {}, (async () => new Response(contents)) as typeof fetch));
    assert.deepEqual(await readFile(destination), contents); assert.deepEqual(await readdir(directory), [file.name]);
  } finally { await rm(directory, {recursive:true, force:true}); }
});

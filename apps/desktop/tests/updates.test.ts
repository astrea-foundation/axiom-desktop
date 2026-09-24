import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {createElement} from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {desktopReleaseFixture} from '../../../fixtures/desktop-releases.mts';
import {UpdateService, type NativeUpdateEvent} from '../src/main/update-service';
import {updateIdentity} from '../src/main/update-identity';
import {compareVersions, FORMATS, parseRelease} from '../../../packages/desktop-releases/manifest.mjs';
import {matchesUpdateTarget, type UpdateIdentity, type UpdateState} from '../src/shared/updates';
import {UpdatesPanel} from '../src/renderer/src/components/UpdatesPanel';

const identity: UpdateIdentity = {currentVersion:'0.1.4',packaged:true,platform:'linux',arch:'x64',format:'AppImage'};
function setup(current = identity, release = desktopReleaseFixture()) {
  const events: UpdateState[] = [], calls: string[][] = [], lifecycle: string[] = [];
  let idle = true;
  const dependencies = {
    run: async (args: string[], receive: (event: NativeUpdateEvent)=>void, signal: AbortSignal) => {
      signal.throwIfAborted(); calls.push(args);
      if (args[0] === '--start-job') {receive({event:'installing'});return;}
      receive({event:'checked',release,installation:{product:'desktop',format:current.format!}});
      if (args[0] === '--prepare' && compareVersions(release.version,current.currentVersion)>0) {
        const file = release.downloads.find(f=>f.format===current.format && f.platform===current.platform && (f.arch===current.arch || f.arch==='universal'))!;
        receive({event:'progress',name:file.name,received:file.bytes,total:file.bytes});
        receive({event:'ready',job:'/private/update/job.json'});
      }
    },
    canRestart:()=>idle,
    prepareRestart: async()=>{lifecycle.push('saved');},
    restart:async()=>{lifecycle.push('restart');},
    parentPid:123,
    emit:(state:UpdateState)=>events.push(state),
  };
  return {service:new UpdateService(current,dependencies),dependencies,events,calls,lifecycle,idle:(value:boolean)=>{idle=value;}};
}
const waitFor = async (predicate:()=>boolean) => {for(let n=0;n<100;n++){if(predicate())return;await new Promise(r=>setTimeout(r,10));}throw new Error('Timed out');};

test('only stamped native installation identities enable updates',async()=>{
  for(const [platform,formats] of Object.entries(FORMATS)) for(const format of formats) {
    const result=updateIdentity({version:'0.1.4',platform:{mac:'darwin',win:'win32',linux:'linux'}[platform]!,arch:'arm64',packaged:true,packageFormat:format});
    assert.equal(result.format,format);
  }
  for(const packaged of [false,true]) {
    const a=setup({...identity,packaged,format:null});
    assert.equal((await a.service.install()).status,'disabled');assert.deepEqual(a.calls,[]);
  }
  assert.equal(updateIdentity({version:'0.1.4',platform:'win32',arch:'x64',packaged:true}).format,null);
});
test('every installation updates its own product, architecture, and package format then restarts',async()=>{
  for(const file of desktopReleaseFixture().downloads) {
    const a=setup({...identity,platform:file.platform,arch:file.arch,format:file.format});
    assert.equal((await a.service.check()).status,'available');
    assert.equal((await a.service.install()).status,'installing');
    assert.deepEqual(a.calls,[['--check'],['--prepare','--desktop'],['--start-job','/private/update/job.json','--parent','123']]);
    assert.deepEqual(a.lifecycle,['saved','restart']);
    assert.ok(a.events.some(s=>s.status==='downloading' && s.download?.name===file.name));
    await a.service.dispose();
  }
});
test('combined signed installers update both Mac and Windows architectures',async()=>{
  const fixture=JSON.parse(readFileSync(new URL('../../../packages/desktop-releases/universal-fixture.json',import.meta.url),'utf8'));
  const release=parseRelease(fixture.release);
  for(const platform of ['mac','win'] as const) for(const arch of ['x64','arm64'] as const) {
    const target={...identity,platform,arch,format:platform==='mac'?'pkg' as const:'exe' as const};
    const file=release.downloads.find(file=>matchesUpdateTarget(file,target));
    assert.ok(file,`${platform}/${arch} has an installer in Desktop`);
    assert.equal(file.arch,'universal');
    const a=setup(target,release);
    const state=await a.service.check();
    const markup=renderToStaticMarkup(createElement(UpdatesPanel,{updates:{
      state,error:null,check:async()=>{},install:async()=>{},cancel:async()=>{},
    }}));
    assert.match(markup,/Update and restart/);
    assert.doesNotMatch(markup,/disabled=""|A matching installer has not been published/);
    assert.equal((await a.service.install()).status,'installing');
    assert.ok(a.events.some(state=>state.status==='downloading' && state.download?.name===file.name));
    assert.deepEqual(a.lifecycle,['saved','restart']);
    await a.service.dispose();
  }
  const mac=release.downloads.find(file=>file.platform==='mac')!;
  assert.equal(matchesUpdateTarget(mac,{...identity,platform:'mac',arch:null,format:'pkg'}),false);
  assert.equal(matchesUpdateTarget(mac,{...identity,platform:'mac',arch:'universal',format:'pkg'}),false);
  assert.equal(matchesUpdateTarget(mac,{...identity,platform:'win',format:'exe'}),false);
  assert.equal(matchesUpdateTarget(mac,{...identity,platform:'mac',format:'exe'}),false);
  const linux=release.downloads.find(file=>file.platform==='linux')!;
  assert.equal(matchesUpdateTarget(linux,{...identity,arch:'arm64'}),false);
});
test('same/older releases do not start installation',async()=>{
  for(const version of ['0.1.4','0.1.3']) {
    const a=setup(identity,desktopReleaseFixture(version));assert.equal((await a.service.install()).status,'current');
    assert.equal(a.calls.length,1);assert.deepEqual(a.lifecycle,[]);
  }
});
test('an active chat or proxy delays handoff, cancellation keeps the app open',async()=>{
  const a=setup();a.idle(false);const operation=a.service.install();
  await waitFor(()=>a.service.snapshot().status==='waiting');
  assert.equal(a.calls.length,1);assert.deepEqual(a.lifecycle,[]);
  assert.equal(a.service.install(),operation,'duplicate actions join one operation');
  a.service.cancel();assert.equal((await operation).status,'available');assert.deepEqual(a.lifecycle,[]);
  a.idle(true);await a.service.install();assert.deepEqual(a.lifecycle,['saved','restart']);
});
test('state save and helper acknowledgement must succeed before shutdown',async()=>{
  for(const stage of ['save','ack']) {
    const a=setup();
    if(stage==='save') a.dependencies.prepareRestart=async()=>{throw new Error('Disk is full');};
    else {const run=a.dependencies.run;a.dependencies.run=async(args,receive,signal)=>{if(args[0]!=='--start-job')await run(args,receive,signal);};}
    assert.equal((await a.service.install()).status,'error');assert.ok(!a.lifecycle.includes('restart'));
  }
});
test('unsigned release, wrong owner, corrupt progress and incomplete download fail closed',async()=>{
  const remote=desktopReleaseFixture();
  const frames: NativeUpdateEvent[][]=[
    [{event:'checked',release:{...remote,signing:'unsigned',signature:undefined},installation:{product:'desktop',format:'AppImage'}}],
    [{event:'checked',release:remote,installation:{product:'cli',format:'sh'}}],
    [{event:'progress',name:'bogus',received:10,total:20}],
    [{event:'checked',release:remote,installation:{product:'desktop',format:'AppImage'}}],
  ];
  for(const events of frames){const a=setup();a.dependencies.run=async(_args,receive)=>{events.forEach(receive);};assert.equal((await a.service.install()).status,'error');assert.deepEqual(a.lifecycle,[]);}
});
test('dispose cancels download and waits for native work to stop',async()=>{
  const a=setup();let stopped=false;
  a.dependencies.run=async(_args,_receive,signal)=>new Promise((_,reject)=>signal.addEventListener('abort',()=>{stopped=true;reject(signal.reason);},{once:true}));
  const operation=a.service.install();await a.service.dispose();await operation;assert.equal(stopped,true);assert.deepEqual(a.lifecycle,[]);
});

import assert from 'node:assert/strict';
import {mkdtemp, mkdir, readFile, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import test from 'node:test';
import {claimFirstLaunchUpdate} from '../src/main/first-launch-update';
import {UpdateService, type NativeUpdateEvent} from '../src/main/update-service';
import {desktopReleaseFixture} from '../../../fixtures/desktop-releases.mts';
import type {UpdateState} from '../src/shared/updates';

test('only installer-marked fresh installs claim a durable attempt, including concurrent/restarted launches', async () => {
  const root = await mkdtemp(join(tmpdir(), 'axiom-first-launch-'));
  const input = {enabled:true, resources:join(root,'resources'), userData:join(root,'user'), executable:'C:\\Users\\Test\\Axiom\\Axiom.exe'};
  try {
    await mkdir(input.resources);
    assert.equal(await claimFirstLaunchUpdate(input),false);
    await writeFile(join(input.resources,'axiom-first-launch.json'),'{"schemaVersion":1}');
    assert.equal(await claimFirstLaunchUpdate({...input,enabled:false}),false);
    const claims = await Promise.all([claimFirstLaunchUpdate(input),claimFirstLaunchUpdate(input)]);
    assert.deepEqual(claims.sort(),[false,true]);
    assert.equal(await claimFirstLaunchUpdate({...input,executable:'c:\\users\\test\\axiom\\Axiom.exe'}),false);
    assert.equal(await claimFirstLaunchUpdate({...input,executable:'C:\\Other\\Axiom.exe'}),true);
    await writeFile(join(input.resources,'axiom-first-launch.json'),'{"schemaVersion":1,"unexpected":true}');
    assert.equal(await claimFirstLaunchUpdate({...input,userData:join(root,'other')}),false);
  } finally {await rm(root,{recursive:true,force:true});}
});

function setup(version='0.1.4', firstLaunch=true, platform='win' as 'win'|'linux') {
  const calls:string[][]=[], states:UpdateState[]=[], lifecycle:string[]=[];
  let finishCheck:(()=>void)|undefined, fail=false, wait=false;
  const deps = {
    run:async (args:string[], receive:(event:NativeUpdateEvent)=>void, signal:AbortSignal) => {
      calls.push(args);
      if(wait && args[0]==='--check') await new Promise<void>((resolve,reject)=>{
        finishCheck=resolve; signal.addEventListener('abort',()=>reject(new Error('Cancelled')),{once:true});
      });
      signal.throwIfAborted();
      if(fail) throw new Error('Feed unavailable');
      if(args[0]==='--start-job') {receive({event:'installing'});return;}
      const release=desktopReleaseFixture(version);
      receive({event:'checked',release,installation:{product:'desktop',format:platform==='win'?'exe':'AppImage'}});
      if(args[0]==='--prepare' && version!=='0.1.4') {
        const file=release.downloads.find(f=>f.platform===platform)!;
        receive({event:'progress',name:file.name,received:file.bytes,total:file.bytes});
        receive({event:'ready',job:'/verified/job.json'});
      }
    }, canRestart:()=>true, prepareRestart:async()=>{lifecycle.push('save');},
    restart:async()=>{lifecycle.push('restart');}, emit:(state:UpdateState)=>states.push(state),parentPid:123,
  };
  return {service:new UpdateService({currentVersion:'0.1.4',platform,arch:'x64',format:platform==='win'?'exe':'AppImage',packaged:true},deps,firstLaunch),calls,states,lifecycle,
    wait:()=>{wait=true;},finish:()=>finishCheck?.(),fail:(value:boolean)=>{fail=value;}};
}

test('fresh Windows launch waits for renderer readiness then checks, verifies and hands off exactly once', async()=>{
  const a=setup('0.1.5');a.wait();a.service.start();
  assert.deepEqual(a.calls,[]);
  const operation=a.service.ready();
  await a.service.ready();
  assert.equal(a.calls.length,1);
  a.finish();await operation;
  assert.deepEqual(a.calls.map(c=>c[0]),['--check','--prepare','--start-job']);
  assert.deepEqual(a.lifecycle,['save','restart']);
  assert.equal(a.service.continue().status,'installing');
  await a.service.dispose();
});
test('current builds proceed without restarting; existing Windows and other platforms keep check-only startup',async()=>{
  const a=setup();a.service.start();await a.service.ready();
  assert.equal(a.service.snapshot().firstLaunch,false);
  assert.deepEqual(a.calls,[['--check']]);assert.deepEqual(a.lifecycle,[]);await a.service.dispose();
  for(const [first,platform] of [[false,'win'],[true,'linux']] as const) {
    const b=setup('0.1.5',first,platform);b.service.start();await b.service.ready();
    await b.service.check();assert.deepEqual(b.calls,[['--check']]);assert.deepEqual(b.lifecycle,[]);
    assert.equal(b.service.snapshot().firstLaunch,undefined);await b.service.dispose();
  }
});
test('offline startup offers retry/continue, and continuing during a check cannot start an installer',async()=>{
  const a=setup('0.1.5');a.fail(true);await a.service.ready();
  assert.equal(a.service.snapshot().status,'error');assert.equal(a.service.snapshot().firstLaunch,true);
  a.fail(false);await a.service.install();assert.deepEqual(a.lifecycle,['save','restart']);await a.service.dispose();
  const b=setup('0.1.5');b.wait();const checking=b.service.ready();b.service.continue();await checking;
  assert.equal(b.service.snapshot().firstLaunch,false);assert.deepEqual(b.calls,[['--check']]);
  assert.deepEqual(b.lifecycle,[]);await b.service.dispose();
});

test('installer marks fresh installs before registry replacement and keeps silent setup from launching Axiom',async()=>{
  const source=await readFile(new URL('../build/installer.nsh',import.meta.url),'utf8');
  assert.match(source,/!macro customInit[\s\S]*ReadRegStr.*INSTALL_REGISTRY_KEY/);
  assert.match(source,/axiomFreshInstall == "1"[\s\S]*axiom-first-launch\.json/);
  assert.doesNotMatch(source,/Exec.*Axiom\.exe/);
});

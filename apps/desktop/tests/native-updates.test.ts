import assert from 'node:assert/strict';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {EventEmitter} from 'node:events';
import {PassThrough} from 'node:stream';
import type {ChildProcessWithoutNullStreams, spawn} from 'node:child_process';
import {nativeUpdates} from '../src/main/native-updates';
import {flushForUpdate,registerUpdateFlush,releaseUpdateFlush,updateRestartPending} from '../src/renderer/src/updateFlush';

function heldHandoff() {
  const stdout=new PassThrough(), stderr=new PassThrough();
  let killed=false;
  const child=Object.assign(new EventEmitter(),{stdout,stderr,kill:()=>{killed=true;return true;}});
  const start=(()=>child as unknown as ChildProcessWithoutNullStreams) as typeof spawn;
  const events:unknown[]=[];
  const operation=nativeUpdates(async()=>'installed-cli',start)(['--start-job','private-job','--parent','123'],event=>events.push(event),new AbortController().signal);
  return {child,stdout,stderr,events,operation,killed:()=>killed};
}

test('handoff completes after acknowledgement and CLI exit even when the helper retains its pipes',async()=>{
  for(const exitFirst of [false,true]) {
    const a=heldHandoff();
    await new Promise<void>(resolve=>setImmediate(resolve));
    let completed=false;void a.operation.then(()=>{completed=true;});
    if(exitFirst) a.child.emit('exit',0,null);
    else a.stdout.write('{"event":"installing"}\n');
    await new Promise<void>(resolve=>setImmediate(resolve));
    assert.equal(completed,false,'both verified helper acknowledgement and successful CLI exit are required');
    if(exitFirst) a.stdout.write('{"event":"installing"}\n');
    else a.child.emit('exit',0,null);
    const timeout=setTimeout(()=>a.child.emit('close',0,null),500);
    try {
      await a.operation;
      assert.equal(a.stdout.destroyed,true,'handoff must release its inherited stdout pipe before helper exit');
      assert.equal(a.stderr.destroyed,true);
      assert.equal(a.killed(),false,'do not terminate the detached installer');
      assert.deepEqual(a.events,[{event:'installing'}]);
    } finally {clearTimeout(timeout);a.stdout.destroy();a.stderr.destroy();}
  }
});

test('handoff rejects missing acknowledgement, failed CLI exit and incomplete trailing output',async()=>{
  for(const stage of ['missing','failed','partial']) {
    const a=heldHandoff();
    await new Promise<void>(resolve=>setImmediate(resolve));
    const rejection=assert.rejects(a.operation);
    if(stage!=='missing') a.stdout.write('{"event":"installing"}\n'+(stage==='partial'?'incomplete':''));
    a.child.emit('exit',stage==='failed'?1:0,null);
    a.child.emit('close',stage==='failed'?1:0,null);
    await rejection;
    a.stdout.destroy();a.stderr.destroy();
  }
});

async function withExecutable(body:string,run:(file:string)=>Promise<void>) {
  const directory=await mkdtemp(path.join(tmpdir(),'axiom-native-updates-'));
  try {const file=path.join(directory,'cli');await writeFile(file,`#!${process.execPath}\n${body}`,{mode:0o755});await run(file);}
  finally{await rm(directory,{recursive:true,force:true});}
}
test('native bridge parses bounded events and excludes account/proxy secrets',{skip:process.platform==='win32'},()=>withExecutable(`
 if(process.env.AXIOM_API_KEY || process.env.AXIOM_PROXY_TOKEN) process.exit(3);
 process.stdout.write('{"event":"checked"}\\n');
`,async file=>{
  const events:unknown[]=[];await nativeUpdates(async()=>file)(['--check'],event=>events.push(event),new AbortController().signal);
  assert.deepEqual(events,[{event:'checked'}]);
}));
test('native bridge rejects oversized, incomplete and failed responses',{skip:process.platform==='win32'},async()=>{
  for(const body of ["process.stdout.write('x'.repeat(300000))", "process.stdout.write('{\"event\":\"checked\"}')", "process.stderr.write('verification failed'); process.exit(1)"]) {
    await withExecutable(body,async file=>{await assert.rejects(nativeUpdates(async()=>file)([],()=>{},new AbortController().signal));});
  }
});
test('native bridge cancellation stops work before it can trigger restart',{skip:process.platform==='win32'},()=>withExecutable("setInterval(()=>{},1000)",async file=>{
  const controller=new AbortController();const operation=nativeUpdates(async()=>file)([],()=>{},controller.signal);
  setTimeout(()=>controller.abort(),30);await assert.rejects(operation);
}));
test('restart waits for durable state and a failed save unfreezes input',async()=>{
  releaseUpdateFlush();let finish!:()=>void;
  const unregister=registerUpdateFlush(()=>new Promise<void>(resolve=>{finish=resolve;}));
  const operation=flushForUpdate();assert.equal(updateRestartPending(),true);finish();await operation;
  unregister();releaseUpdateFlush();
  const fail=registerUpdateFlush(async()=>{throw new Error('Disk full');});
  await assert.rejects(flushForUpdate(),/Disk full/);assert.equal(updateRestartPending(),false);fail();
});

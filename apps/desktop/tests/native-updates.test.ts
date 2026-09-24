import assert from 'node:assert/strict';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {nativeUpdates} from '../src/main/native-updates';
import {flushForUpdate,registerUpdateFlush,releaseUpdateFlush,updateRestartPending} from '../src/renderer/src/updateFlush';

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

import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { AxiomAcpClient } from "../src/client.js";

function fixture(t: TestContext) {
  const client=new AxiomAcpClient({command:process.execPath,args:["-e","process.stdin.resume()"]});
  t.after(()=>client.close());
  client.state.setAccount({revision:1,state:"valid",account:{id:"account-a",linkedMethods:[]}});
  client.state.addSession("thread","/tmp");
  Object.assign(client,{supportsImageRegeneration:true});
  let refreshed=0;
  Object.assign(client,{refreshThread:async()=>{refreshed++;}});
  const requests:{method:string;params:unknown}[]=[];
  let finish!:(value:{callId:string})=>void;
  t.mock.method(client.process,"request",async(method:string,params:unknown)=>{
    requests.push({method,params});
    return new Promise<{callId:string}>(resolve=>{finish=resolve;});
  });
  return {client,requests,finish:()=>finish({callId:"new-image"}),refreshed:()=>refreshed};
}

test("regenerating one image never resends a prompt and serializes the owning conversation",async t=>{
  const f=fixture(t);
  const pending=f.client.regenerateImage("thread","original-image");
  assert.equal(f.client.getState().sessions.thread?.running,true);
  await assert.rejects(f.client.regenerateImage("thread","original-image"),/current response/);
  assert.deepEqual(f.requests,[{method:"_axiom/image/regenerate",params:{threadId:"thread",callId:"original-image"}}]);
  f.finish();
  assert.deepEqual(await pending,{callId:"new-image"});
  assert.equal(f.client.getState().sessions.thread?.running,false);
  assert.equal(f.refreshed(),1);
});

test("an old account's image result cannot refresh the newly active account",async t=>{
  const f=fixture(t);
  const pending=f.client.regenerateImage("thread","original-image");
  const checked=assert.rejects(pending,/account|context/i);
  f.client.state.setAccount({revision:2,state:"valid",account:{id:"account-b",linkedMethods:[]}});
  f.finish();await checked;
  assert.equal(f.refreshed(),0);
  assert.equal(f.client.getState().sessions.thread,undefined);
});

test("an older native client fails before any generation or prompt is sent",async t=>{
  const f=fixture(t);Object.assign(f.client,{supportsImageRegeneration:false});
  await assert.rejects(f.client.regenerateImage("thread","original-image"),/Restart Axiom/);
  assert.deepEqual(f.requests,[]);
});

test("Stop cancels image work even before the native handler has registered a running turn",async t=>{
  const f=fixture(t);
  t.mock.method(f.client.process,"notify",async()=>{});
  t.mock.method(f.client.process,"request",async(_method:string,_params:unknown,_timeout:number,signal:AbortSignal)=>new Promise((_resolve,reject)=>{
    signal.addEventListener("abort",()=>reject(new Error("cancelled")),{once:true});
  }));
  const pending=assert.rejects(f.client.regenerateImage("thread","original-image"),/cancelled/);
  await f.client.cancel("thread");await pending;
  assert.equal(f.client.getState().sessions.thread?.running,false);
});

test("a snapshot begun during regeneration cannot leave a completed conversation running",async t=>{
  const f=fixture(t);
  Object.assign(f.client,{refreshThread:AxiomAcpClient.prototype.refreshThread});
  let finishOld!:()=>void;
  let snapshots=0;
  t.mock.method(f.client as any,"fetchTimeline",async()=>{
    if(++snapshots===1) {
      await new Promise<void>(resolve=>{finishOld=resolve;});
      f.client.state.setRunning("thread",true);
    } else f.client.state.setRunning("thread",false);
    return {};
  });
  const old=f.client.refreshThread("thread");
  const regeneration=f.client.regenerateImage("thread","original-image");
  f.finish();
  await Promise.resolve();
  finishOld();await old;await regeneration;
  assert.equal(snapshots,2,"completion must fetch state after an older snapshot settles");
  assert.equal(f.client.getState().sessions.thread?.running,false);
});

for (const editing of [false,true]) test(`completion after ${editing?"editing":"sending"} cannot reuse a running snapshot`,async t=>{
  const f=fixture(t);
  Object.assign(f.client,{refreshThread:AxiomAcpClient.prototype.refreshThread,supportsMessageRevision:true});
  let finishOld!:()=>void;
  let finishPrompt!:(value:{stopReason:string})=>void;
  let snapshots=0;
  t.mock.method(f.client as any,"fetchTimeline",async()=>{
    if(++snapshots===1) {
      await new Promise<void>(resolve=>{finishOld=resolve;});
      f.client.state.setRunning("thread",true);
    } else f.client.state.setRunning("thread",false);
    return {};
  });
  t.mock.method(f.client.process,"request",async(method:string)=>{
    assert.equal(method,"session/prompt");
    return new Promise(resolve=>{finishPrompt=resolve;});
  });
  const old=f.client.refreshThread("thread");
  const prompt=f.client.prompt("thread","New complete instruction","new-message",false,0,
    editing?{userItemId:"original-user-message",expectedRevision:1}:undefined);
  await new Promise(resolve=>setImmediate(resolve));
  finishPrompt({stopReason:"end_turn"});await Promise.resolve();
  finishOld();await old;await prompt;
  assert.equal(snapshots,2);
  assert.equal(f.client.getState().sessions.thread?.running,false);
});

test("lazy image reads stay outside shared state and reject delayed account responses", async t => {
  const f = fixture(t); Object.assign(f.client, { supportsImageResources: true });
  let finish!: (value: unknown) => void;
  const response = { data: "cG5n", mimeType: "image/png", width: 1024, height: 1024, sha256: "ab".repeat(32) };
  t.mock.method(f.client.process, "request", async (method: string, params: unknown) => {
    assert.equal(method, "_axiom/image/read");
    assert.deepEqual(params, { threadId: "thread", callId: "image", sha256: response.sha256 });
    return new Promise(resolve => { finish = resolve; });
  });
  const first = f.client.readImage("thread", "image", response.sha256); finish(response);
  assert.deepEqual(await first, response);
  assert.ok(!JSON.stringify(f.client.getState()).includes(response.data));
  const late = f.client.readImage("thread", "image", response.sha256);
  const rejected = assert.rejects(late, /account|context/i);
  f.client.state.setAccount({ revision: 2, state: "valid", account: { id: "account-b", linkedMethods: [] } });
  finish(response); await rejected;
});

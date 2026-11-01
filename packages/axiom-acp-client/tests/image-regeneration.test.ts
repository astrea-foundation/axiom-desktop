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

import assert from "node:assert/strict";
import test from "node:test";
import { AxiomAcpClient } from "../src/client.js";

for (const supported of [false, true]) {
  test(`local payload extension ${supported ? "binds requests and replies to the active account" : "fails closed with an old native app"}`, async t => {
    const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
    t.after(() => client.close());
    let calls = 0;
    let finish!: (value: unknown) => void;
    t.mock.method(client.process, "request", async (method: string) => {
      if (method === "initialize") return {
        protocolVersion: 1, agentCapabilities: { _meta: { axiom: {
          protocolVersion: "0.2", runtimeInstanceId: "fixture",
          features: { desktopChat: 1, threadCatalog: 1, timeline: 2, modelCatalog: 1,
            profilePreferences: 1, collections: 1, account: 2, billing: 4, securityEvidence: 4, webConsent: 1,
            localPayload: supported ? 1 : 0 },
        } } },
      };
      assert.equal(method, "_axiom/desktop/local-payload"); calls++;
      return new Promise(resolve => { finish = resolve; });
    });
    await client.initialize();
    client.state.setAccount({ revision: 1, state: "valid", account: { id: "alice", linkedMethods: [] } });
    if (!supported) {
      await assert.rejects(client.localPayload({ accountId: "alice", id: "draft", action: { kind: "get" } }), /updated native app/);
      assert.equal(calls, 0); return;
    }
    await assert.rejects(client.localPayload({ accountId: "bob", id: "draft", action: { kind: "remove" } }), /account changed/);
    assert.equal(calls, 0);
    const pending = client.localPayload({ accountId: "alice", id: "draft", action: { kind: "get" } });
    assert.equal(calls, 1);
    client.state.setAccount({ revision: 2, state: "valid", account: { id: "bob", linkedMethods: [] } });
    client.state.setAccount({ revision: 3, state: "valid", account: { id: "alice", linkedMethods: [] } });
    finish({ value: "old owner response" });
    await assert.rejects(pending, /account|connection/i);
  });
}

for (const version of [1, 2]) {
  test(`legacy imports require localPayload@2 while ordinary persistence works at version ${version}`, async t => {
    const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
    t.after(() => client.close());
    const calls: string[] = [];
    let finish!: (value: unknown) => void;
    let delay = false;
    t.mock.method(client.process, "request", async (method: string, request?: { action: { kind: string } }) => {
      if (method === "initialize") return {
        protocolVersion: 1, agentCapabilities: { _meta: { axiom: {
          protocolVersion: "0.2", runtimeInstanceId: "fixture",
          features: { desktopChat: 1, threadCatalog: 1, timeline: 2, modelCatalog: 1,
            profilePreferences: 1, collections: 1, account: 2, billing: 4, securityEvidence: 4, webConsent: 1, localPayload: version },
        } } },
      };
      assert.equal(method, "_axiom/desktop/local-payload"); calls.push(request!.action.kind);
      if (delay) return new Promise(resolve => { finish = resolve; });
      return { value: { version: 1, kind: "prompt", payload: { text: "normalized", attachments: [] } }, omittedMedia: 1 };
    });
    await client.initialize();
    client.state.setAccount({ revision: 1, state: "valid", account: { id: "alice", linkedMethods: [] } });
    await client.localPayload({ accountId: "alice", id: "draft", action: { kind: "get" } });
    const request = { accountId: "alice", id: "draft", action: { kind: "import_legacy" as const, value: { version: 1, kind: "prompt", payload: { text: "raw", attachments: [] } }, preview: true } };
    if (version === 1) {
      await assert.rejects(client.localPayload(request), /updated native app.*original local data was kept/i);
      assert.deepEqual(calls, ["get"]);
    } else {
      assert.equal((await client.localPayload(request)).omittedMedia, 1);
      assert.deepEqual(calls, ["get", "import_legacy"]);
      delay = true;
      const pending = client.localPayload(request);
      client.state.setAccount({ revision: 2, state: "valid", account: { id: "bob", linkedMethods: [] } });
      finish({ value: "stale normalized source", omittedMedia: 1 });
      await assert.rejects(pending, /account|connection/i);
    }
  });
}

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

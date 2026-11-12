import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import type { AgentApi } from "../src/preload/agent-api";
import { AgentStateImages } from "../src/preload/agent-state-images";
import { AgentStateTransport } from "../src/main/agent-state-transport";
import type { ClientState } from "@axiom/axiom-acp-client";

function fixture() {
  let owner = "alice";
  const transport = new AgentStateTransport();
  const packet = () => transport.encode({ connected: true, runtimeInstanceId: "runtime", lastSequence: 0,
    account: { state: "valid", account: { id: owner } }, sessions: {}, catalog: [], collections: { revision: 0, collections: [] }, preferences: null, billing: null, diagnostic: "" } as unknown as ClientState, true);
  const listeners = new Map<string, (event: unknown, packet: unknown) => void>();
  const calls: unknown[][] = [];
  let complete!: (value: unknown) => void;
  const source = readFileSync(new URL("../src/preload/agent-api.ts", import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } });
  const exports: { agentApi?: AgentApi } = {};
  runInNewContext(outputText, {
    exports, window: { addEventListener: () => {} },
    require: (name: string) => {
      if (name === "./agent-state-images") return { AgentStateImages };
      assert.equal(name, "electron");
      return { ipcRenderer: {
        invoke: async (channel: string, ...args: unknown[]) => {
          calls.push([channel, ...args]);
          if (channel === "agent:state") return packet();
          return new Promise(resolve => { complete = resolve; });
        },
        on: (name: string, callback: (event: unknown, packet: unknown) => void) => listeners.set(name, callback),
        removeListener: () => {},
      } };
    },
  });
  return { api: exports.agentApi!, calls, complete: (value: unknown) => complete(value),
    switchAccount(account: string) { owner = account; listeners.get("agent:state-changed")!(null, packet()); } };
}

test("preload requires its current account and discards an old-owner reply after away-and-back", async () => {
  const f = fixture();
  await f.api.getState();
  await assert.rejects(f.api.localPayload({ accountId: "bob", id: "draft", action: { kind: "get" } }), /account changed/);
  assert.equal(f.calls.length, 1);
  const pending = f.api.localPayload({ accountId: "alice", id: "draft", action: { kind: "get" } });
  assert.equal(f.calls[1]?.[0], "agent:local-payload");
  f.switchAccount("bob"); f.switchAccount("alice");
  f.complete({ value: { version: 1, kind: "prompt", payload: { text: "old account response", attachments: [] } } });
  await assert.rejects(pending, /account changed/);
});

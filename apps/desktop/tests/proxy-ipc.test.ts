import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import test from "node:test";
import ts from "typescript";
import type { ProxyApi } from "../src/shared/proxy";

test("proxy preload exposes scoped controls and a clipboard action without requesting credentials", async () => {
  const source = readFileSync(new URL("../src/preload/proxy-api.ts", import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const calls: unknown[][] = [];
  const exports: { proxyApi?: ProxyApi } = {};
  runInNewContext(outputText, { exports, require: (name: string) => {
    assert.equal(name, "electron");
    return { ipcRenderer: { invoke: async (...args: unknown[]) => { calls.push(args); } } };
  } });
  const api = exports.proxyApi!;
  await api.getState();
  await api.start(8484, "account", "runtime");
  assert.equal(await api.copyToken("account", "runtime"), undefined);
  await api.stop();
  assert.deepEqual(calls, [
    ["proxy:state"], ["proxy:start", 8484, "account", "runtime"],
    ["proxy:copy-token", "account", "runtime"], ["proxy:stop"],
  ]);
});

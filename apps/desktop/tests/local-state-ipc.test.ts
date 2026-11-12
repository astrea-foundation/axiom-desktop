import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import ts from "typescript";

function source(path: string) {
  return ts.transpileModule(readFileSync(new URL(path, import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test("main flush accepts only its window/main-frame/token ACK and times out without leaked listeners", async () => {
  const ipc = new EventEmitter(), sent: unknown[][] = [];
  const frame = {}, contents = { mainFrame: frame, send: (...args: unknown[]) => sent.push(args) };
  let timeout!: () => void, cleared = 0;
  const exports: any = {};
  runInNewContext(source("../src/main/update-lifecycle.ts"), {
    exports,
    require: (name: string) => name === "node:crypto" ? { randomUUID: () => "token" }
      : { ipcMain: ipc, BrowserWindow: { getAllWindows: () => [{ webContents: contents, isDestroyed: () => false }] } },
    setTimeout: (callback: () => void, delay: number) => { assert.equal(delay, 10_000); timeout = callback; return 1; },
    clearTimeout: () => { cleared++; },
  });
  let completed = false;
  const save = exports.prepareLocalState("account").then(() => { completed = true; });
  assert.deepEqual(sent, [["local-state:prepare", "token", "account"]]);
  ipc.emit("local-state:flushed", { sender: {}, senderFrame: frame }, "token", null);
  ipc.emit("local-state:flushed", { sender: contents, senderFrame: {} }, "token", null);
  ipc.emit("local-state:flushed", { sender: contents, senderFrame: frame }, "wrong", null);
  await tick(); assert.equal(completed, false);
  ipc.emit("local-state:flushed", { sender: contents, senderFrame: frame }, "token", null);
  await save; assert.equal(cleared, 1); assert.equal(ipc.listenerCount("local-state:flushed"), 0);
  const hung = exports.prepareLocalState();
  timeout();
  await assert.rejects(hung, /10 seconds/);
  assert.equal(ipc.listenerCount("local-state:flushed"), 0);
  exports.releaseLocalState(); assert.equal(sent.at(-1)?.[0], "local-state:release");
});

test("preload sends a local flush ACK only after completion and releases on cancellation", async () => {
  const ipc = Object.assign(new EventEmitter(), { send: (...args: unknown[]) => { sent.push(args); }, invoke: async () => ({}) });
  const sent: unknown[][] = [];
  let api: any;
  runInNewContext(source("../src/preload/index.ts"), {
    exports: {}, process: { argv: [], platform: "linux" },
    require: (name: string) => {
      if (name === "electron") return { ipcRenderer: ipc, contextBridge: { exposeInMainWorld: (_name: string, value: unknown) => { api = value; } } };
      if (name === "./agent-api") return { agentApi: {} };
      if (name === "./proxy-api") return { proxyApi: {} };
      throw Error(name);
    },
  });
  let finish!: () => void, releases = 0;
  const unsubscribe = api.onBeforeLocalStateChange((reason: string) => {
    assert.equal(reason, "close"); return new Promise<void>(resolve => { finish = resolve; });
  }, () => { releases++; });
  ipc.emit("local-state:prepare", null, "token", "close");
  await tick(); assert.deepEqual(sent, []);
  finish(); await tick(); assert.deepEqual(sent, [["local-state:flushed", "token", null]]);
  ipc.emit("local-state:release"); assert.equal(releases, 1);
  unsubscribe(); assert.equal(ipc.listenerCount("local-state:prepare"), 0);
  assert.equal(ipc.listenerCount("local-state:release"), 0);
});

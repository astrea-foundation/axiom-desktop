import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const preload = fileURLToPath(new URL("../out/preload/index.cjs", import.meta.url));

test("real Electron skips never-mounted windows but still protects a registered renderer after reload", {
  // Opt in after building Desktop. This is a hidden, temporary Electron profile
  // with no native sidecar, credentials, application data or live network.
  skip: process.env.AXIOM_NATIVE_LIFECYCLE_TEST !== "1" || !existsSync(preload), timeout: 25_000,
}, async () => {
  const root = await mkdtemp(join(tmpdir(), "axiom-local-state-"));
  const require = createRequire(import.meta.url);
  const electron = require("electron") as string;
  const environment = { ...process.env }; delete environment.ELECTRON_RUN_AS_NODE;
  const entry = join(root, "app.cjs"), lifecycle = join(root, "lifecycle.cjs"), resultPath = join(root, "result.json");
  const source = await readFile(new URL("../src/main/update-lifecycle.ts", import.meta.url), "utf8");
  await writeFile(lifecycle, ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS,
    target: ts.ScriptTarget.ES2022 } }).outputText);
  await writeFile(entry, `const {app, BrowserWindow, ipcMain} = require('electron');
const fs = require('node:fs'), assert = require('node:assert/strict');
const {prepareLocalState, prepareUpdateRestart} = require(${JSON.stringify(lifecycle)});
app.setName('Axiom Local State Test');
app.setPath('userData', ${JSON.stringify(join(root, "profile"))});
app.on('window-all-closed', () => {});
app.whenReady().then(async () => {
  const windows = [], results = {};
  try {
    // No app has mounted in this window, as with a missing development server.
    const blank = new BrowserWindow({show:false}); windows.push(blank);
    const started = Date.now();
    await prepareLocalState(); await prepareUpdateRestart();
    results.blankDuration = Date.now() - started;
    assert(results.blankDuration < 1000);
    blank.destroy();

    const ready = new BrowserWindow({show:false, webPreferences:{preload:${JSON.stringify(preload)}, contextIsolation:true, sandbox:true}});
    windows.push(ready);
    await ready.loadURL('data:text/html,<html><body>Isolated local state fixture</body></html>');
    const registered = new Promise(resolve => {
      const listener = event => {if(event.sender === ready.webContents){ipcMain.removeListener('local-state:ready', listener); resolve();}};
      ipcMain.on('local-state:ready', listener);
    });
    await ready.webContents.executeJavaScript(
      "window.axiomDesktop.onBeforeLocalStateChange(async () => {window.flushCount=(window.flushCount||0)+1;if(window.failFlush)throw Error('fixture save failure');}, () => {});" +
      "window.axiomDesktop.updates.onBeforeRestart(async () => {window.updateCount=(window.updateCount||0)+1;}); true");
    await registered;
    await prepareLocalState('account');
    await prepareUpdateRestart();
    results.flushCount = await ready.webContents.executeJavaScript('window.flushCount');
    results.updateCount = await ready.webContents.executeJavaScript('window.updateCount');
    assert.equal(results.flushCount, 1); assert.equal(results.updateCount, 1);
    await ready.webContents.executeJavaScript('window.failFlush=true');
    await assert.rejects(prepareLocalState(), /drafts or queued messages could not be saved/);
    results.failedSaveProtected = true;

    // Reload removes the callback but must not erase knowledge of an existing
    // writer. Losing the app is never permission to skip its draft barrier.
    await ready.loadURL('about:blank');
    const lostAt = Date.now();
    await assert.rejects(prepareLocalState(), /Saving local data did not finish/);
    results.reloadDuration = Date.now() - lostAt;
    assert(results.reloadDuration >= 9000);
    fs.writeFileSync(${JSON.stringify(resultPath)}, JSON.stringify(results));
    for(const window of windows) if(!window.isDestroyed()) window.destroy();
    app.exit(0);
  } catch(error) {
    fs.writeFileSync(${JSON.stringify(resultPath)}, JSON.stringify({error:String(error), stack:error.stack}));
    app.exit(1);
  }
});
`);
  let diagnostics = "";
  const child = spawn(electron, [entry], { env: environment, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  child.stderr.on("data", chunk => { diagnostics = (diagnostics + chunk).slice(-4096); });
  const timer = setTimeout(() => child.kill("SIGKILL"), 22_000);
  try {
    const exit = await new Promise<number | null>((resolve, reject) => {
      child.once("exit", resolve); child.once("error", reject);
    });
    const result = existsSync(resultPath) ? JSON.parse(await readFile(resultPath, "utf8")) : null;
    assert.equal(exit, 0, JSON.stringify(result) + diagnostics);
    assert.equal(result?.error, undefined, JSON.stringify(result) + diagnostics);
    assert.equal(result.flushCount, 1);
    assert.equal(result.updateCount, 1);
    assert.equal(result.failedSaveProtected, true);
    assert(result.blankDuration < 1000);
    assert(result.reloadDuration >= 9000);
  } finally {
    clearTimeout(timer); child.kill("SIGKILL");
    await rm(root, { recursive: true, force: true });
  }
});

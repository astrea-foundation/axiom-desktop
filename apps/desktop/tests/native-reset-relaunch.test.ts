import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { checkFactoryResetSupport, factoryResetError, factoryResetRelaunch } from "../src/main/factory-reset";

const binary = process.env.AXIOM_RESET_TEST_BINARY;

test("real Electron exits, launches the native cleanup helper, then reopens with a visible failure result", {
  // Opt in: this starts a separate, windowless Electron with a temporary
  // profile. An invalid deletion path forces failure BEFORE credentials/files
  // are accessed. Never run a real factory reset against the developer's data.
  skip: !binary || !existsSync(binary), timeout: 25_000,
}, async () => {
  const root = await mkdtemp(join(tmpdir(), "axiom-reset-relaunch-"));
  const require = createRequire(import.meta.url);
  const electron = require("electron") as string;
  const environment = { ...process.env }; delete environment.ELECTRON_RUN_AS_NODE;
  const entry = join(root, "app.cjs"), restarted = join(root, "restarted.cjs");
  const resultPath = join(root, "result.json");
  const userData = join(root, "unowned-profile");
  await mkdir(userData);
  const relaunch = factoryResetRelaunch({ sidecar: resolve(binary!), executable: process.execPath,
    argv: [electron, restarted, root], userData, profile: "system" });
  await writeFile(restarted, `const fs = require('node:fs'), path = require('node:path');
const root = process.argv[2];
const pid = Number(fs.readFileSync(path.join(root, 'parent'), 'utf8'));
let parentAlive = true; try { process.kill(pid, 0); } catch { parentAlive = false; }
fs.writeFileSync(path.join(root, 'result.json'), JSON.stringify({parentAlive, argv: process.argv}));
`);
  await writeFile(entry, `const {app} = require('electron'), fs = require('node:fs');
app.setName('Axiom Factory Reset Test');
app.setPath('userData', ${JSON.stringify(userData)});
app.whenReady().then(() => {
  fs.writeFileSync(${JSON.stringify(join(root, "parent"))}, String(process.pid));
  app.relaunch(${JSON.stringify(relaunch)});
  app.exit(0);
});
`);
  let diagnostics = "";
  const child = spawn(electron, [entry], { env: environment, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  child.stderr.on("data", (chunk) => { diagnostics = (diagnostics + chunk).slice(-4096); });
  const exited = new Promise<number | null>((resolve, reject) => {
    child.once("exit", resolve); child.once("error", reject);
  });
  try {
    await checkFactoryResetSupport(resolve(binary!));
    const exit = await exited;
    assert.equal(exit, 0, diagnostics);
    const deadline = Date.now() + 15_000;
    while (!existsSync(resultPath) && Date.now() < deadline) await delay(100);
    assert.equal(existsSync(resultPath), true, `native cleanup did not reopen the fixture: ${diagnostics}`);
    const result = JSON.parse(await readFile(resultPath, "utf8"));
    assert.equal(result.parentAlive, false, "cleanup/restart must happen after Electron exits");
    assert.match(factoryResetError(result.argv) ?? "", /not a standard Axiom profile/);
    assert.equal(existsSync(userData), true, "validation failure must not delete the temporary profile");
  } finally {
    child.kill();
    await rm(root, { recursive: true, force: true });
  }
});

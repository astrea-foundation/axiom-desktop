import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { _electron } from "playwright";

assert.equal(process.platform, "win32", "Run with a native Windows Electron build");
const require = createRequire(import.meta.url);
const root = await mkdtemp(join(tmpdir(), "axiom-interface-size-"));
const profile = join(root, "profile");
const entry = join(root, "launch.mjs");
const sidecar = fileURLToPath(new URL("../../../target/debug/axiomcli.exe", import.meta.url));
await mkdir(profile);
await writeFile(entry, [
  'import { app } from "electron";',
  'app.setPath("userData", ' + JSON.stringify(profile) + ');',
  'app.on("browser-window-created", (_, window) => {',
  '  window.once("ready-to-show", () => {',
  '    globalThis.initialWindow = { zoom: window.webContents.getZoomFactor(), size: window.getSize() };',
  '  });',
  '});',
  'await import(' + JSON.stringify(new URL("../out/main/index.js", import.meta.url).href) + ');',
].join("\n"));

const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  !key.startsWith("AXIOM") && key !== "ELECTRON_RUN_AS_NODE" && key !== "ELECTRON_RENDERER_URL"));
Object.assign(env, {
  AXIOM_AUTH_URL: "http://127.0.0.1:9",
  AXIOM_BASE_URL: "https://127.0.0.1:9",
  AXIOMCLI_SIDECAR: sidecar,
  XDG_CONFIG_HOME: join(root, "config"),
  XDG_DATA_HOME: join(root, "data"),
});
const launch = () => _electron.launch({executablePath: require("electron"), args: [entry], env});
const selectedSize = (page, size) => page.waitForFunction(async value =>
  (await window.axiomDesktop.interfaceSize.get()).size === value, size);

let app;
try {
  app = await launch();
  assert.equal(await app.evaluate(({app}) => app.getPath("userData")), profile);
  const page = await app.firstWindow();
  page.setDefaultTimeout(10000);
  await selectedSize(page, 110);
  await page.getByRole("button", {name: "Account menu", exact: true}).click();
  const initial = await app.evaluate(() => globalThis.initialWindow);
  assert(Math.abs(initial.zoom - 1.1) < 0.001);
  assert.deepEqual(await app.evaluate(({BrowserWindow}) => BrowserWindow.getAllWindows()[0].getSize()), initial.size);

  await page.getByRole("menuitem", {name: "Settings", exact: true}).click();
  const choices = page.getByRole("radiogroup", {name: "Interface size"});
  const shortcut = async (key, size) => {
    await app.evaluate(({BrowserWindow}, keyCode) => {
      const window = BrowserWindow.getAllWindows()[0];
      window.focus();
      window.webContents.focus();
      for (const type of ["keyDown", "keyUp"]) window.webContents.sendInputEvent({
        type, keyCode, modifiers: ["control"],
      });
    }, key);
    await selectedSize(page, size);
    await choices.getByRole("radio", {checked: true, name: new RegExp("^" + size + "%")}).waitFor();
  };
  await shortcut("=", 125);
  await shortcut("-", 110);
  await shortcut("=", 125);
  await shortcut("0", 110);
  await page.getByRole("button", {name: "Back", exact: true}).click();

  for (const size of [125, 150]) {
    await page.evaluate(value => window.axiomDesktop.interfaceSize.set(value), size);
    const title = page.locator("header.drag-region").first();
    await title.waitFor();
    assert.equal(await title.evaluate(node => {
      const style = getComputedStyle(node);
      return style.getPropertyValue("app-region") || style.getPropertyValue("-webkit-app-region");
    }), "drag");
    await page.getByRole("button", {name: "Maximize", exact: true}).click();
    await page.getByRole("button", {name: "Restore", exact: true}).click();
    await page.getByRole("button", {name: "Maximize", exact: true}).waitFor();
  }
  await selectedSize(page, 150);
  await app.close();
  app = undefined;
  assert.equal(JSON.parse(await readFile(join(profile, "interface-size.json"), "utf8")).size, 150);
  app = await launch();
  const reopened = await app.firstWindow();
  await selectedSize(reopened, 150);
  await reopened.getByRole("button", {name: "Account menu", exact: true}).waitFor();
  assert(Math.abs((await app.evaluate(() => globalThis.initialWindow)).zoom - 1.5) < 0.001);
  console.log("Windows interface size, accelerators, frameless controls and persisted startup passed.");
} finally {
  await app?.close();
  await rm(root, {recursive: true, force: true});
}

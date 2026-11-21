import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { InterfaceSizeController, readInterfaceSize, type SizedWindow } from "../src/main/interface-size";
import {
  defaultInterfaceSize, interfaceSizeShortcut, INTERFACE_SIZES, isInterfaceSize, minimumWindowSize, stepInterfaceSize,
  type InterfaceSizeKey, type InterfaceSizeState,
} from "../src/shared/interface-size";

const key = (key: string, modifiers: Partial<InterfaceSizeKey> = {}): InterfaceSizeKey =>
  ({ type: "keyDown", key, control: false, meta: false, alt: false, ...modifiers });

function fakeWindow(size = [1320, 860]) {
  const listeners = new Map<string, Function[]>();
  const record = { zoom: [] as number[], sent: [] as InterfaceSizeState[], minimum: [] as number[][], size, prevented: 0 };
  const on = (event: string, listener: Function) => { listeners.set(event, [...(listeners.get(event) ?? []), listener]); };
  const window: SizedWindow = {
    isDestroyed: () => false, isMaximized: () => false, isFullScreen: () => false,
    getSize: () => record.size, setSize: (width, height) => { record.size = [width, height]; },
    setMinimumSize: (width, height) => { record.minimum.push([width, height]); },
    once: on,
    webContents: {
      setZoomFactor: (factor) => { record.zoom.push(factor); },
      send: (_channel, state) => { record.sent.push(state); },
      on,
    },
  };
  const emit = (event: string, ...args: unknown[]) => { for (const listener of listeners.get(event) ?? []) listener(...args); };
  const press = (input: InterfaceSizeKey) => emit("before-input-event", { preventDefault: () => { record.prevented++; } }, input);
  return { window, record, emit, press };
}

test("Windows and Linux start one step larger than macOS", () => {
  assert.equal(defaultInterfaceSize("darwin"), 100);
  assert.equal(defaultInterfaceSize("win32"), 110);
  assert.equal(defaultInterfaceSize("linux"), 110);
  assert.deepEqual([...INTERFACE_SIZES], [100, 110, 125, 150]);
  assert.equal(isInterfaceSize(125), true);
  for (const value of [90, 115, "110", null, 1.1]) assert.equal(isInterfaceSize(value), false);
});

test("stepping moves through the offered sizes and stops at either end", () => {
  assert.equal(stepInterfaceSize(100, 1), 110);
  assert.equal(stepInterfaceSize(110, 1), 125);
  assert.equal(stepInterfaceSize(150, 1), 150);
  assert.equal(stepInterfaceSize(125, -1), 110);
  assert.equal(stepInterfaceSize(100, -1), 100);
});

test("shortcuts need the platform's own modifier and ignore a held key", () => {
  for (const plus of ["+", "="]) assert.equal(interfaceSizeShortcut(key(plus, { control: true }), "win32"), "larger");
  assert.equal(interfaceSizeShortcut(key("-", { control: true }), "linux"), "smaller");
  assert.equal(interfaceSizeShortcut(key("0", { control: true }), "win32"), "reset");
  assert.equal(interfaceSizeShortcut(key("Add", { control: true, code: "NumpadAdd" }), "win32"), "larger");
  assert.equal(interfaceSizeShortcut(key("Subtract", { control: true, code: "NumpadSubtract" }), "win32"), "smaller");
  assert.equal(interfaceSizeShortcut(key("=", { meta: true }), "darwin"), "larger");
  assert.equal(interfaceSizeShortcut(key("=", { control: true }), "darwin"), null, "Ctrl is not the macOS shortcut key");
  assert.equal(interfaceSizeShortcut(key("=", { meta: true }), "win32"), null, "the Windows key is not a shortcut key");
  assert.equal(interfaceSizeShortcut(key("=", { control: true, meta: true }), "win32"), null);
  assert.equal(interfaceSizeShortcut(key("=", { control: true, alt: true }), "win32"), null, "AltGr combinations type characters");
  assert.equal(interfaceSizeShortcut(key("=", { control: true, isAutoRepeat: true }), "win32"), null);
  assert.equal(interfaceSizeShortcut({ ...key("=", { control: true }), type: "keyUp" }, "win32"), null);
  assert.equal(interfaceSizeShortcut(key("=") , "win32"), null);
  assert.equal(interfaceSizeShortcut(key("a", { control: true }), "win32"), null);
});

test("only the largest size raises the smallest window", () => {
  for (const size of [100, 110, 125] as const) assert.deepEqual(minimumWindowSize(size), { width: 960, height: 640 });
  assert.deepEqual(minimumWindowSize(150), { width: 1080, height: 720 });
});

test("the stored choice survives a restart, and an unreadable file falls back to the platform default", async () => {
  const userData = await mkdtemp(join(tmpdir(), "axiom-interface-size-"));
  try {
    assert.equal(readInterfaceSize(userData, "win32"), 110);
    assert.equal(readInterfaceSize(userData, "darwin"), 100);
    const controller = new InterfaceSizeController(userData, "win32");
    assert.deepEqual(controller.state, { size: 110, defaultSize: 110 });
    assert.deepEqual(controller.set(125), { size: 125, defaultSize: 110 });
    await controller.saved();
    assert.deepEqual(JSON.parse(await readFile(join(userData, "interface-size.json"), "utf8")), { schemaVersion: 1, size: 125 });
    assert.equal(new InterfaceSizeController(userData, "win32").zoomFactor, 1.25);
    assert.equal(readInterfaceSize(userData, "darwin"), 125, "a stored choice outranks the platform default");
    assert.throws(() => controller.set(115), /Unsupported interface size/);
    assert.throws(() => controller.set("150"), /Unsupported interface size/);
    for (const broken of ["{", JSON.stringify({ size: 115 }), JSON.stringify(125), "null"]) {
      await writeFile(join(userData, "interface-size.json"), broken);
      assert.equal(readInterfaceSize(userData, "win32"), 110);
    }
  } finally { await rm(userData, { recursive: true, force: true }); }
});

test("a window opens at the stored size and every later document is set again", async () => {
  const userData = await mkdtemp(join(tmpdir(), "axiom-interface-size-"));
  try {
    const controller = new InterfaceSizeController(userData, "win32");
    const { window, record, emit } = fakeWindow();
    controller.attach(window);
    assert.deepEqual(record.zoom, [1.1]);
    assert.deepEqual(record.minimum, [[960, 640]]);
    // Chromium restores its own remembered zoom as a document commits.
    emit("did-navigate"); emit("dom-ready");
    assert.deepEqual(record.zoom, [1.1, 1.1, 1.1]);
    assert.deepEqual(record.sent.at(-1), { size: 110, defaultSize: 110 });
  } finally { await rm(userData, { recursive: true, force: true }); }
});

test("shortcuts step every open window, tell the renderer, and reset to the platform default", async () => {
  const userData = await mkdtemp(join(tmpdir(), "axiom-interface-size-"));
  try {
    const controller = new InterfaceSizeController(userData, "win32");
    const first = fakeWindow(), second = fakeWindow();
    controller.attach(first.window); controller.attach(second.window);
    first.press(key("=", { control: true }));
    assert.equal(first.record.prevented, 1);
    for (const { record } of [first, second]) {
      assert.equal(record.zoom.at(-1), 1.25);
      assert.deepEqual(record.sent.at(-1), { size: 125, defaultSize: 110 });
    }
    first.press(key("=", { control: true })); first.press(key("=", { control: true }));
    assert.equal(controller.state.size, 150, "the largest size is the limit");
    first.press(key("0", { control: true }));
    assert.equal(controller.state.size, 110);
    first.press(key("-", { control: true })); first.press(key("-", { control: true }));
    assert.equal(controller.state.size, 100);
    first.press(key("a", { control: true }));
    assert.equal(first.record.prevented, 6, "other keys reach the page");
    second.emit("closed");
    const sentToClosed = second.record.sent.length;
    controller.set(125);
    assert.equal(second.record.sent.length, sentToClosed, "a closed window is no longer addressed");
    await controller.saved();
    assert.equal(readInterfaceSize(userData, "win32"), 125);
  } finally { await rm(userData, { recursive: true, force: true }); }
});

test("the largest size grows a window that is smaller than its new minimum, except when maximized", async () => {
  const userData = await mkdtemp(join(tmpdir(), "axiom-interface-size-"));
  try {
    const controller = new InterfaceSizeController(userData, "darwin");
    const small = fakeWindow([960, 640]);
    const maximized = fakeWindow([960, 640]);
    maximized.window.isMaximized = () => true;
    controller.attach(small.window); controller.attach(maximized.window);
    controller.set(150);
    assert.deepEqual(small.record.minimum.at(-1), [1080, 720]);
    assert.deepEqual(small.record.size, [1080, 720]);
    assert.deepEqual(maximized.record.size, [960, 640]);
    controller.set(100);
    assert.deepEqual(small.record.minimum.at(-1), [960, 640]);
    assert.deepEqual(small.record.size, [1080, 720], "a smaller minimum never shrinks the window");
  } finally { await rm(userData, { recursive: true, force: true }); }
});

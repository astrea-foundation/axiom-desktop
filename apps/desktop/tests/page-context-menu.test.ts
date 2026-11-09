import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { basename } from "node:path";
import test from "node:test";
import type { BrowserWindow, ContextMenuParams, SaveDialogOptions } from "electron";
import { installPageContextMenu, pageContextMenu } from "../src/main/page-context-menu";

const textContext = {
  x: 123, y: 234,
  mediaType: "none", hasImageContents: false, srcURL: "", isEditable: false,
  selectionText: "Selected tool or thinking text",
  editFlags: { canCopy: true, canCut: false, canPaste: false, canSelectAll: true },
} as ContextMenuParams;

test("selected text gets native Copy and local images offer native copy and save", () => {
  const downloads: string[] = [];
  const copies: [number, number][] = [];
  const contents = {
    copyImageAt: (x: number, y: number) => { copies.push([x, y]); },
    downloadURL: (url: string) => { downloads.push(url); },
  };
  assert.deepEqual(pageContextMenu(textContext, contents), [{ role: "copy", enabled: true }]);
  for (const url of ["blob:file:///local-image", "data:image/png;base64,cG5n"]) {
    const items = pageContextMenu({ ...textContext, mediaType: "image", hasImageContents: true, srcURL: url, selectionText: "" }, contents);
    assert.deepEqual(items.map(item => item.label), ["Copy image", "Save image as…"]);
    Reflect.apply(items[0]!.click!, undefined, []);
    Reflect.apply(items[1]!.click!, undefined, []);
  }
  assert.deepEqual(copies, [[123, 234], [123, 234]]);
  assert.deepEqual(downloads, ["blob:file:///local-image", "data:image/png;base64,cG5n"]);
  for (const url of ["https://example.invalid/image.png", "file:///private/image.png", "javascript:alert(1)"]) {
    assert.deepEqual(pageContextMenu({ ...textContext, mediaType: "image", hasImageContents: true, srcURL: url, selectionText: "" }, contents), []);
  }
  assert.deepEqual(pageContextMenu({ ...textContext, mediaType: "image", hasImageContents: false, srcURL: "blob:file:///broken-image", selectionText: "" }, contents), []);
  const editable = pageContextMenu({ ...textContext, isEditable: true }, contents);
  assert.deepEqual(editable.filter(item => item.role).map(item => item.role), ["cut", "copy", "paste", "selectAll"]);
});

test("local image downloads get fresh filenames and remain scoped to their owning window", () => {
  const session = new EventEmitter();
  const contents = Object.assign(new EventEmitter(), { session, copyImageAt: () => {}, downloadURL: () => {} });
  const window = { webContents: contents } as unknown as BrowserWindow;
  const popups: unknown[] = [];
  installPageContextMenu(window, "/downloads", () => ({ popup: options => { popups.push(options); } }));
  contents.emit("context-menu", {}, textContext);
  assert.deepEqual(popups, [{ window }]);
  const options: SaveDialogOptions[] = [];
  const item = (url = "blob:file:///local-image", mime = "image/png") => ({
    getURL: () => url, getMimeType: () => mime, setSaveDialogOptions: (value: SaveDialogOptions) => options.push(value),
  });
  session.emit("will-download", {}, item(), contents);
  session.emit("will-download", {}, item(), contents);
  assert.equal(options.length, 2);
  for (const option of options) assert.match(basename(option.defaultPath!), /^[0-9a-f-]{36}\.png$/);
  assert.notEqual(options[0]?.defaultPath, options[1]?.defaultPath);
  session.emit("will-download", {}, item(), new EventEmitter());
  session.emit("will-download", {}, item("https://example.invalid/image.png"), contents);
  session.emit("will-download", {}, item("blob:file:///text-file", "text/plain"), contents);
  assert.equal(options.length, 2);
  contents.emit("destroyed");
  assert.equal(session.listenerCount("will-download"), 0);
});

import { randomUUID } from "node:crypto";
import { join } from "node:path";
import type { BrowserWindow, ContextMenuParams, DownloadItem, Event, Menu, MenuItemConstructorOptions, WebContents } from "electron";

const imageExtensions: Record<string, string> = {
  "image/png": "png", "image/jpeg": "jpg", "image/webp": "webp", "image/gif": "gif",
};

function localImageUrl(url: string): boolean {
  return url.startsWith("blob:") || /^data:image\/(png|jpeg|webp|gif);base64,/i.test(url);
}

type PageContext = Pick<ContextMenuParams, "x" | "y" | "mediaType" | "hasImageContents" | "srcURL" | "isEditable" | "selectionText" | "editFlags">;

export function pageContextMenu(params: PageContext, contents: Pick<WebContents, "copyImageAt" | "downloadURL">): MenuItemConstructorOptions[] {
  const items: MenuItemConstructorOptions[] = [];
  if (params.mediaType === "image" && params.hasImageContents && localImageUrl(params.srcURL)) {
    // Chromium keeps the image in this document's session. No image bytes,
    // renderer file paths or additional network fetches cross an IPC endpoint.
    items.push(
      { label: "Copy image", click: () => contents.copyImageAt(params.x, params.y) },
      { label: "Save image as…", click: () => contents.downloadURL(params.srcURL) },
    );
  }
  if (params.isEditable || params.selectionText) {
    if (items.length) items.push({ type: "separator" });
    if (params.isEditable) items.push({ role: "cut", enabled: params.editFlags.canCut });
    items.push({ role: "copy", enabled: params.editFlags.canCopy });
    if (params.isEditable) items.push(
      { role: "paste", enabled: params.editFlags.canPaste },
      { type: "separator" },
      { role: "selectAll", enabled: params.editFlags.canSelectAll },
    );
  }
  return items;
}

export function installPageContextMenu(
  window: BrowserWindow,
  downloadsDirectory: string,
  createMenu: (items: MenuItemConstructorOptions[]) => Pick<Menu, "popup">,
): void {
  const contents = window.webContents;
  const session = contents.session;
  const download = (_event: Event, item: DownloadItem, owner: WebContents) => {
    if (owner !== contents || !localImageUrl(item.getURL())) return;
    const extension = imageExtensions[item.getMimeType()];
    if (!extension) return;
    // Both the result's Save link and the native context menu use a fresh name
    // for each save. Electron still asks the user for the destination.
    item.setSaveDialogOptions({ defaultPath: join(downloadsDirectory, `${randomUUID()}.${extension}`) });
  };
  session.on("will-download", download);
  contents.once("destroyed", () => session.removeListener("will-download", download));
  contents.on("context-menu", (_event, params) => {
    const items = pageContextMenu(params, contents);
    if (items.length) createMenu(items).popup({ window });
  });
}

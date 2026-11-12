import {BrowserWindow, ipcMain} from 'electron';
import {randomUUID} from 'node:crypto';

/** The renderer must commit its local drafts before the installer may start. */
export async function prepareUpdateRestart(): Promise<void> {
  return prepare('updates:prepare-restart', 'updates:flushed');
}

export async function prepareLocalState(reason: "close" | "account" = "close"): Promise<void> {
  return prepare('local-state:prepare', 'local-state:flushed', reason);
}

export function releaseLocalState(): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed()) window.webContents.send('local-state:release');
  }
}

async function prepare(requestChannel: string, responseChannel: string, reason?: "close" | "account"): Promise<void> {
  await Promise.all(BrowserWindow.getAllWindows().map(window => new Promise<void>((resolve,reject) => {
    const token = randomUUID();
    const cleanup = () => { clearTimeout(timer); ipcMain.removeListener(responseChannel,acknowledge); };
    const acknowledge = (event: Electron.IpcMainEvent, id: unknown, error: unknown) => {
      if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame || id !== token) return;
      cleanup();
      if (error) reject(new Error('Your drafts or queued messages could not be saved. Keep Axiom open, unlock local storage if needed, and retry.'));
      else resolve();
    };
    const timer = setTimeout(() => { cleanup(); reject(new Error('Saving local data did not finish within 10 seconds. Axiom stayed open. Unlock local storage if needed, then retry.')); },10000);
    ipcMain.on(responseChannel,acknowledge);
    window.webContents.send(requestChannel,token,reason);
  })));
}

import {BrowserWindow, ipcMain} from 'electron';
import {randomUUID} from 'node:crypto';

/** The renderer must commit its local drafts before the installer may start. */
export async function prepareUpdateRestart(): Promise<void> {
  await Promise.all(BrowserWindow.getAllWindows().map(window => new Promise<void>((resolve,reject) => {
    const token = randomUUID();
    const cleanup = () => { clearTimeout(timer); ipcMain.removeListener('updates:flushed',acknowledge); };
    const acknowledge = (event: Electron.IpcMainEvent, id: unknown, error: unknown) => {
      if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame || id !== token) return;
      cleanup();
      if (error) reject(new Error('Your drafts could not be saved. The update has not started.'));
      else resolve();
    };
    const timer = setTimeout(() => { cleanup(); reject(new Error('Axiom could not save local state before restarting. Try again.')); },10000);
    ipcMain.on('updates:flushed',acknowledge);
    window.webContents.send('updates:prepare-restart',token);
  })));
}

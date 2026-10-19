import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { app, dialog } from 'electron';

const execute = promisify(execFile);

/** Optional command registration for users who installed the drag-and-drop DMG. */
export async function installMacCli(): Promise<void> {
  if (process.platform !== 'darwin' || !app.isPackaged) return;
  if (process.resourcesPath !== '/Applications/Axiom.app/Contents/Resources') {
    await dialog.showMessageBox({ type: 'info', message: 'Move Axiom to Applications first.', detail: 'Quit Axiom, copy it to /Applications, then open that copy and install the command-line tool.' });
    return;
  }
  const script = [
    'set -eu',
    'target=/Applications/Axiom.app/Contents/Resources/bin/axiomcli',
    'link=/usr/local/bin/axiomcli',
    'test -x "$target"',
    'if [ -e "$link" ] || [ -L "$link" ]; then [ -L "$link" ] && [ "$(readlink "$link")" = "$target" ] || { echo "Another axiomcli is already installed at $link." >&2; exit 1; }; fi',
    'mkdir -p /usr/local/bin',
    'ln -sfn "$target" "$link"',
  ].join('\n');
  // AppleScript string escaping, followed by its standard native elevation prompt.
  const literal = '"' + script.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n') + '"';
  try {
    await execute('/usr/bin/osascript', ['-e', `do shell script ${literal} with administrator privileges`]);
    await dialog.showMessageBox({ type: 'info', message: 'Command-line tool installed', detail: 'Open Terminal and run axiomcli.' });
  } catch {
    await dialog.showMessageBox({ type: 'error', message: 'Command-line tool installation did not complete.', detail: 'Authorize the installation when prompted. If /usr/local/bin/axiomcli belongs to another installation, relocate it first.' });
  }
}

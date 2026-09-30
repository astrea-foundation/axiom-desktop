import {createHash} from 'node:crypto';
import {mkdir, readFile, writeFile} from 'node:fs/promises';
import {join, win32} from 'node:path';

/** The installer marks fresh installs; a durable claim prevents restart loops. */
export async function claimFirstLaunchUpdate(input: {
  enabled: boolean; resources: string; userData: string; executable: string;
}): Promise<boolean> {
  if (!input.enabled) return false;
  try {
    const marker = JSON.parse(await readFile(join(input.resources, 'axiom-first-launch.json'), 'utf8'));
    if (marker?.schemaVersion !== 1 || Object.keys(marker).length !== 1) return false;
    const installation = win32.normalize(win32.dirname(input.executable)).toLowerCase();
    const key = createHash('sha256').update(installation).digest('hex');
    const directory = join(input.userData, 'first-launch-updates');
    await mkdir(directory, {recursive: true});
    // Claim before networking or restart. Explicit retries remain available in UI.
    await writeFile(join(directory, `${key}.json`), JSON.stringify({schemaVersion: 1, installation}), {flag: 'wx', mode: 0o600});
    return true;
  } catch {
    // No marker, prior launch, unreadable storage or malformed marker: normal UI.
    // This metadata controls presentation only; it grants no update authority.
    return false;
  }
}

import {mkdtemp, readFile, readdir, rm, writeFile} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import packageLicenses from './package-licenses.cjs';

// Reuse the locked Rust inventory already distributed with the native apps.
// The standalone EXE embeds this bundle instead of needing adjacent files.
export async function writeSetupNotices(workspace, destination) {
  const temporary = await mkdtemp(path.join(os.tmpdir(), 'axiom-setup-notices-'));
  try {
    await packageLicenses(workspace, temporary);
    const files = (await readdir(temporary, {recursive:true, withFileTypes:true}))
      .filter(entry => entry.isFile())
      .map(entry => path.relative(temporary, path.join(entry.parentPath, entry.name)))
      .sort();
    const sections = ['Axiom Setup — project license and third-party notices\n\nThis bundle includes the locked workspace Rust dependency inventory. Some components are used by other Axiom applications.'];
    for (const file of files) sections.push(`\n\n===== ${file.split(path.sep).join('/')} =====\n\n${await readFile(path.join(temporary,file),'utf8')}`);
    // The published GUI crate omits its repository license file.
    for (const file of ['LICENSE', 'NOTICE.txt']) sections.push(`\n\n===== native-windows-gui-1.0.13/${file} =====\n\n${await readFile(path.join(workspace,'apps/axiom-setup/resources/licenses/native-windows-gui',file),'utf8')}`);
    await writeFile(destination, sections.join(''));
  } finally {
    await rm(temporary, {recursive:true, force:true});
  }
}

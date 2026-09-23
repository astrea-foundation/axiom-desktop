import {execFileSync} from 'node:child_process';
import {mkdir} from 'node:fs/promises';
import path from 'node:path';
import {verifyNativeInput} from './native-input.mjs';
import assertArchitecture from './native-binary.cjs';

const [input, output] = process.argv.slice(2);
if (process.platform !== 'darwin' || !input || !output) throw new Error('Usage on macOS: combine-macos.mjs INPUT OUTPUT');
for (const arch of ['x64','arm64']) await verifyNativeInput(path.join(input, arch), 'mac', arch);
await mkdir(output, {recursive:true});
for (const name of ['axiomcli','axiom-proxy']) {
  const destination = path.join(output, name);
  execFileSync('lipo', ['-create', path.join(input,'x64',name), path.join(input,'arm64',name), '-output',destination], {stdio:'inherit'});
  assertArchitecture(destination, 'mac', 'universal');
}

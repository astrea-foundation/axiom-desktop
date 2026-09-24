const { execFileSync } = require('node:child_process');
const { copyFile, cp, mkdir, readFile, readdir, writeFile } = require('node:fs/promises');
const path = require('node:path');

// Rust's MSVC targets use the dynamic C runtime. Ship the redistributable DLLs
// beside the CLI so a clean Windows installation needs no separate VC installer.
module.exports = async function bundleWindowsRuntime(resources, preparedRoot) {
  const bin = path.join(resources, 'bin');
  const cli = await readFile(path.join(bin, 'axiomcli.exe'));
  const machine = cli.readUInt16LE(cli.readUInt32LE(0x3c) + 4);
  const arch = { 0x8664: 'x64', 0xaa64: 'arm64' }[machine];
  if (!arch) throw new Error('Unsupported Windows CLI architecture');
  if (preparedRoot) {
    const source = path.join(preparedRoot, arch, 'runtime');
    const libraries = await readdir(path.join(source, 'bin'));
    if (!libraries.some(name=>name.toLowerCase() === 'vcruntime140.dll')) throw new Error('Incomplete captured Windows runtime');
    require('../../../scripts/native-binary.cjs')(path.join(source, 'bin/vcruntime140.dll'), 'win', arch);
    for (const name of libraries) {
      // Keep the complete vendor runtime set. Microsoft's ARM64 redist also
      // contains a companion DLL with an x64 PE header; the primary runtime
      // above must match the CLI, and the build receipt verifies every file.
      await copyFile(path.join(source, 'bin', name), path.join(bin, name));
    }
    await cp(path.join(source, 'licenses'), path.join(resources, 'licenses'), {recursive:true});
    return;
  }
  const vswhere = path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft Visual Studio/Installer/vswhere.exe');
  const installation = execFileSync(vswhere, ['-latest', '-products', '*', '-property', 'installationPath'], { encoding: 'utf8' }).trim();
  if (!installation) throw new Error('Visual Studio C++ redistributable files are required for Windows packaging');
  const redist = path.join(installation, 'VC/Redist/MSVC');
  const versions = (await readdir(redist)).sort((a, b) => b.localeCompare(a, 'en', { numeric: true }));
  let source;
  for (const version of versions) {
    const architecture = path.join(redist, version, arch);
    const folders = await readdir(architecture).catch(() => []);
    const runtime = folders.find(name => /^Microsoft\.VC\d+\.CRT$/.test(name));
    if (runtime) { source = path.join(architecture, runtime); break; }
  }
  if (!source) throw new Error(`Visual Studio has no redistributable C runtime for ${arch}`);
  const libraries = (await readdir(source)).filter(name => name.toLowerCase().endsWith('.dll'));
  if (!libraries.some(name => name.toLowerCase() === 'vcruntime140.dll')) throw new Error('Incomplete Visual C++ runtime');
  for (const library of libraries) await copyFile(path.join(source, library), path.join(bin, library));
  const notices = path.join(resources, 'licenses');
  await mkdir(notices, { recursive: true });
  await writeFile(path.join(notices, 'microsoft-runtime.txt'), [
    'Microsoft Visual C++ Redistributable runtime components',
    'Copyright Microsoft Corporation. All rights reserved.',
    'These app-local files are copied from the Visual Studio redistributable directory.',
    'https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files',
    ...libraries,
    '',
  ].join('\n'));
};

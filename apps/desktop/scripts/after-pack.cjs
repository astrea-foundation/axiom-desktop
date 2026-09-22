const { copyFile, chmod, mkdir, writeFile } = require('node:fs/promises');
const path = require('node:path');

module.exports = async function afterPack(context) {
  const root = context.packager.projectDir;
  const workspace = path.resolve(root, '../..');
  const resources = context.packager.getResourcesDir(context.appOutDir);
  const format = context.packager.config.extraMetadata?.axiomUpdateFormat;
  if (!['exe', 'pkg', 'AppImage', 'deb', 'pacman'].includes(format)) throw new Error('Package must declare its update installation format');
  await mkdir(resources, {recursive: true});
  await writeFile(path.join(resources, 'axiom-install.json'), JSON.stringify({schemaVersion: 1, product: 'desktop', format, versioned: false}) + '\n');
  await require('../../../scripts/package-licenses.cjs')(workspace, path.join(resources, 'licenses'));
  if (context.electronPlatformName === 'win32') {
    await mkdir(path.join(resources, 'launchers'), {recursive: true});
    for (const file of ['axiomcli.cmd', 'axiomcli.ps1']) await copyFile(path.join(root, 'build/cli', file), path.join(resources, 'launchers', file));
    // The NSIS decoder bundled with electron-builder 26.15 cannot read modern
    // 7-Zip's automatic BCJ2/ARM64 filters and silently skips executable files.
    // https://github.com/electron-userland/electron-builder/issues/9983
    process.env.ELECTRON_BUILDER_7Z_FILTER = 'BCJ';
    await require('./bundle-windows-runtime.cjs')(resources);
  }
  if (context.electronPlatformName === 'linux') {
    // electron-builder copies appOutDir into the AppImage after creating AppRun.
    // Our launcher supports the CLI without starting Electron and retains its sandbox.
    await copyFile(path.join(root, 'build/AppRun'), path.join(context.appOutDir, 'AppRun'));
    await chmod(path.join(context.appOutDir, 'AppRun'), 0o755);
    await mkdir(resources, { recursive: true });
    await copyFile(path.join(root, 'resources/icon.png'), path.join(resources, 'icon.png'));
    await copyFile(path.join(root, 'scripts/install-appimage.sh'), path.join(resources, 'install-appimage.sh'));
    await chmod(path.join(resources, 'install-appimage.sh'), 0o755);
  }
};

const { copyFile, chmod, mkdir, readdir, writeFile } = require('node:fs/promises');
const { execFileSync } = require('node:child_process');
const path = require('node:path');

module.exports = async function afterPack(context) {
  const root = context.packager.projectDir;
  const workspace = path.resolve(root, '../..');
  const resources = context.packager.getResourcesDir(context.appOutDir);
  const notices = path.join(resources, 'licenses');
  await mkdir(notices, { recursive: true });
  for (const file of ['LICENSE', 'THIRD_PARTY.md']) {
    await copyFile(path.join(workspace,file),path.join(notices,file));
    await chmod(path.join(notices,file),0o644);
  }
  const metadata = JSON.parse(execFileSync('cargo', ['metadata','--locked','--format-version','1'], { cwd:workspace, encoding:'utf8', maxBuffer:32*1024*1024 }));
  const inventory = [];
  for (const pkg of metadata.packages) {
    const source = path.dirname(pkg.manifest_path);
    const destination = path.join(notices,'rust',`${pkg.name}-${pkg.version}`);
    // Native crates carry additional licenses inside their vendored C sources.
    const recursive = /-(sys|src)$/.test(pkg.name) || pkg.name === 'ring';
    const entries = (await readdir(source, { withFileTypes:true, recursive }))
      .filter(entry => entry.isFile() && /^(LICENSE|COPYING|NOTICE)/i.test(entry.name) && !/\.(rs|c|h|cpp|js|ts|py)$/i.test(entry.name))
      .map(entry => path.relative(source,path.join(entry.parentPath,entry.name)))
      .sort();
    for (const entry of entries) {
      await mkdir(path.dirname(path.join(destination,entry)),{recursive:true});
      await copyFile(path.join(source,entry),path.join(destination,entry));
      // Cargo sources can contain group-only licenses. The installed app is
      // root-owned, so every user (and codesign verification) must be able to read them.
      await chmod(path.join(destination,entry),0o644);
    }
    inventory.push({ name:pkg.name, version:pkg.version, license:pkg.license, files:entries.map(entry=>entry.split(path.sep).join('/')) });
  }
  await writeFile(path.join(notices,'rust-dependencies.json'),JSON.stringify(inventory,null,2)+'\n');
  if (context.electronPlatformName === 'win32') {
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

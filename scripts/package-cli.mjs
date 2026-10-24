import {copyFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile, chmod} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {verifyNativeInput} from './native-input.mjs';
import {spawnSync} from 'node:child_process';

const root = fileURLToPath(new URL('..', import.meta.url));
const args = process.argv.slice(2);
const flags = new Set(['--linux','--win','--mac','--x64','--arm64','--universal','--unsigned']);
for (let i=0;i<args.length;i++) {
  if (['--input','--output'].includes(args[i])) { if (!args[++i] || args[i].startsWith('--')) throw new Error('Missing directory argument'); }
  else if (!flags.has(args[i])) throw new Error(`Unknown packaging argument: ${args[i]}`);
}
if (args.filter(a=>['--linux','--win','--mac'].includes(a)).length !== 1 || args.filter(a=>['--x64','--arm64','--universal'].includes(a)).length !== 1) throw new Error('Choose one platform and architecture');
const arch = args.includes('--universal') ? 'universal' : args.includes('--arm64') ? 'arm64' : 'x64';
const platform = args.includes('--win') ? 'win' : args.includes('--mac') ? 'mac' : 'linux';
const unsigned = args.includes('--unsigned');
if (arch === 'universal' && platform === 'linux') throw new Error('Linux installers are architecture-specific');
const inputIndex = args.indexOf('--input'), outputIndex = args.indexOf('--output');
const input = inputIndex >= 0 ? path.resolve(args[inputIndex + 1]) : path.join(root, 'target/release');
const output = outputIndex >= 0 ? path.resolve(args[outputIndex + 1]) : path.join(root, 'apps/desktop/dist');
const version = /^version\s*=\s*"([^"]+)"/m.exec(await readFile(path.join(root, 'apps/axiomcli/Cargo.toml'), 'utf8'))[1];
if (!unsigned && !process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('A release update trust key is required');
if ({win32:'win',darwin:'mac',linux:'linux'}[process.platform] !== platform) throw new Error('Build installers on their native OS');
function run(program, values, options = {}) {
  const result = spawnSync(program, values, {stdio: 'inherit', ...options});
  if (result.error || result.status !== 0) throw result.error ?? new Error(`${program} failed (${result.status})`);
  return result;
}
const temporary = await mkdtemp(path.join(tmpdir(), 'axiom-cli-package-'));
try {
  const format = {linux:'sh',mac:'pkg',win:'exe'}[platform];
  const payload = path.join(temporary, 'payload');
  const signScript = path.join(root,'apps/desktop/scripts/sign-windows.ps1');
  async function prepare(arch, input, payload) {
    await mkdir(path.join(payload, 'bin'), {recursive:true}); await mkdir(output, {recursive:true});
    const extension = platform === 'win' ? '.exe' : '';
    for (const name of ['axiomcli', 'axiom-proxy']) {
      const binary = `${name}${extension}`;
      await copyFile(path.join(input, binary), path.join(payload, 'bin', binary));
      await chmod(path.join(payload, 'bin', binary), 0o755);
      createRequire(import.meta.url)('./native-binary.cjs')(path.join(payload,'bin',binary),platform,arch);
    }
    const cli = path.join(payload, 'bin', `axiomcli${extension}`);
    if (arch === process.arch || (arch === 'universal' && platform === 'mac')) {
      const reported = run(cli, ['--version'], {encoding:'utf8', stdio:'pipe'}).stdout.trim();
      if (reported !== `axiomcli ${version}`) throw new Error('CLI product version mismatch');
      if (!unsigned && run(cli, ['update','--trust-keys'], {encoding:'utf8',stdio:'pipe'}).stdout.trim() !== process.env.AXIOM_UPDATE_PUBLIC_KEYS) throw new Error('Compiled update trust key mismatch');
    }
    if (arch !== process.arch && arch !== 'universal') await verifyNativeInput(input, platform, arch);
    await writeFile(path.join(payload, 'axiom-install.json'), JSON.stringify({schemaVersion:1, product:'cli', format, versioned:platform==='linux'})+'\n');
    await createRequire(import.meta.url)('./package-licenses.cjs')(root, path.join(payload, 'licenses'));
    if (platform !== 'win') {
      await copyFile(path.join(root,'scripts/cli-uninstall.sh'),path.join(payload,'uninstall.sh'));
      await chmod(path.join(payload,'uninstall.sh'),0o755);
    }
    if (platform === 'win') {
      const require = createRequire(import.meta.url);
      await require('../apps/desktop/scripts/bundle-windows-runtime.cjs')(payload, process.env.AXIOM_NATIVE_INPUT);
      await mkdir(path.join(payload,'launchers'),{recursive:true});
      for (const name of ['axiomcli.cmd','axiomcli.ps1','axiom-proxy.cmd']) await copyFile(path.join(root,'apps/desktop/build/cli',name),path.join(payload,'launchers',name));
      await copyFile(path.join(root,'apps/desktop/build/windows-cli-path.ps1'),path.join(payload,'windows-cli-path.ps1'));
      if (!unsigned) {
        for (const folder of ['bin','launchers']) for (const name of await readdir(path.join(payload,folder))) {
          if (/\.(exe|dll|ps1)$/.test(name)) run('pwsh',['-NoProfile','-NonInteractive','-File',signScript,'-FilePath',path.join(payload,folder,name)]);
        }
        run('pwsh',['-NoProfile','-NonInteractive','-File',signScript,'-FilePath',path.join(payload,'windows-cli-path.ps1')]);
      }
    }
  }
  if (platform === 'win' && arch === 'universal') {
    process.env.AXIOM_NATIVE_INPUT = input;
    for (const nativeArch of ['x64','arm64']) {
      await verifyNativeInput(path.join(input,nativeArch), platform, nativeArch);
      await prepare(nativeArch, path.join(input,nativeArch), path.join(payload,nativeArch));
    }
  } else {
    await prepare(arch, input, payload);
  }
  const destination = path.join(output, `AxiomCLI-${version}-${platform}-${arch}.${format}`);
  if (platform === 'linux') {
    const archive = path.join(temporary,'payload.tar.gz');
    run('tar', ['-czf',archive,'-C',payload,'.']);
    const header = (await readFile(path.join(root,'scripts/cli-installer.sh.in'),'utf8')).replaceAll('@VERSION@',version);
    await writeFile(destination, header + (await readFile(archive)).toString('base64').replace(/.{1,76}/g,'$&\n'));
    await chmod(destination,0o755);
  } else if (platform === 'win') {
    const compiler = process.env.AXIOM_NSIS ?? path.join(process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)', 'NSIS/makensis.exe');
    run(compiler, [...(arch === 'universal' ? [`/DINPUT_X64=${path.join(payload,'x64')}`, `/DINPUT_ARM64=${path.join(payload,'arm64')}`] : [`/DINPUT=${payload}`]),`/DOUTPUT=${destination}`,`/DVERSION=${version}`, ...(!unsigned ? [`/DSIGN_SCRIPT=${signScript}`] : []),path.join(root,'scripts/cli-installer.nsi')]);
  } else {
    const packageRoot = path.join(temporary,'root');
    const installed = path.join(packageRoot,'usr/local/lib/axiom-cli');
    await mkdir(path.dirname(installed),{recursive:true});
    await import('node:fs/promises').then(fs=>fs.cp(payload,installed,{recursive:true}));
    const scripts = path.join(temporary,'scripts'); await mkdir(scripts);
    await writeFile(path.join(scripts,'preinstall'), '#!/bin/sh\nset -eu\nfor n in axiomcli axiom-proxy; do p=/usr/local/bin/$n; if [ -e "$p" ] || [ -L "$p" ]; then [ -L "$p" ] && [ "$(readlink "$p")" = "/usr/local/lib/axiom-cli/bin/$n" ] || { echo "$p belongs to another installation" >&2; exit 1; }; fi; done\n');
    await writeFile(path.join(scripts,'postinstall'), '#!/bin/sh\nset -eu\nmkdir -p /usr/local/bin\nfor n in axiomcli axiom-proxy; do ln -sfn /usr/local/lib/axiom-cli/bin/$n /usr/local/bin/$n; done\n');
    for (const name of ['preinstall','postinstall']) await chmod(path.join(scripts,name),0o755);
    let identities = '';
    if (!unsigned) identities = run('security',['find-identity','-v'],{stdio:'pipe',encoding:'utf8'}).stdout;
    const identity = kind => {
      const match = [...identities.matchAll(/"([^"\n]+)"/g)].map(m=>m[1]).find(name=>name.startsWith(`Developer ID ${kind}:`) && name.endsWith(`(${process.env.APPLE_TEAM_ID})`));
      if (!match) throw new Error(`Developer ID ${kind} signing identity is missing`); return match;
    };
    if (!unsigned) for (const name of ['axiomcli','axiom-proxy']) run('codesign',['--force','--options','runtime','--timestamp','--sign',identity('Application'),path.join(installed,'bin',name)]);
    run('pkgbuild',['--root',packageRoot,'--scripts',scripts,'--identifier','stream.axiom.cli','--version',version,...(!unsigned ? ['--sign',identity('Installer'),'--timestamp'] : []),destination]);
    if (!unsigned) {
      run('xcrun',['notarytool','submit',destination,'--apple-id',process.env.APPLE_ID,'--password',process.env.APPLE_APP_SPECIFIC_PASSWORD,'--team-id',process.env.APPLE_TEAM_ID,'--wait']);
      run('xcrun',['stapler','staple',destination]); run('pkgutil',['--check-signature',destination]);
    }
  }
  console.log(destination);
} finally { await rm(temporary,{recursive:true,force:true}); }

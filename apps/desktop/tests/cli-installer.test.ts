import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdtempSync,mkdirSync,readFileSync,writeFileSync,copyFileSync,rmSync,readlinkSync,existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import test from 'node:test';

const root=fileURLToPath(new URL('../../..',import.meta.url));
test('standalone installer activates CLI and proxy together, preserves old version/data, rejects damaged upgrades and uninstalls', {skip:process.platform!=='linux'},()=>{
  const directory=mkdtempSync(path.join(tmpdir(),'axiom installer $literal '));
  const prefix=path.join(directory,'installed'), commands=path.join(directory,'commands');
  const env={...process.env,XDG_BIN_HOME:commands};
  function installer(version:string,reported=version,suffix='') {
    const payload=path.join(directory,version);mkdirSync(path.join(payload,'bin'),{recursive:true});mkdirSync(path.join(payload,'licenses'),{recursive:true});
    writeFileSync(path.join(payload,'bin/axiomcli'),`#!/bin/sh\nprintf 'axiomcli ${reported}\\n'\n${suffix}`,{mode:0o755});
    writeFileSync(path.join(payload,'bin/axiom-proxy'),`#!/bin/sh\nprintf 'proxy ${version}\\n'\n`,{mode:0o755});
    writeFileSync(path.join(payload,'axiom-install.json'),JSON.stringify({schemaVersion:1,product:'cli',format:'sh',versioned:true}));
    copyFileSync(path.join(root,'scripts/cli-uninstall.sh'),path.join(payload,'uninstall.sh'));
    const archive=spawnSync('tar',['-czf','-','-C',payload,'.']);assert.equal(archive.status,0);
    const file=path.join(directory,`${version}.sh`);
    writeFileSync(file,readFileSync(path.join(root,'scripts/cli-installer.sh.in'),'utf8').replaceAll('@VERSION@',version)+archive.stdout.toString('base64')+'\n');return file;
  }
  const install=(file:string)=>spawnSync('/bin/sh',[file,'--prefix',prefix],{env,encoding:'utf8'});
  const run=(name:string)=>spawnSync(path.join(commands,name),['--version'],{env,encoding:'utf8'}).stdout.trim();
  try {
    const account=path.join(directory,'account.sqlite');writeFileSync(account,'preserved account data');
    let result=install(installer('1.0.0'));assert.equal(result.status,0,result.stderr);
    assert.equal(run('axiomcli'),'axiomcli 1.0.0');assert.equal(run('axiom-proxy'),'proxy 1.0.0');
    result=install(installer('1.0.1'));assert.equal(result.status,0,result.stderr);
    assert.equal(run('axiomcli'),'axiomcli 1.0.1');assert.equal(run('axiom-proxy'),'proxy 1.0.1');
    assert.equal(readlinkSync(path.join(prefix,'current')),'versions/1.0.1');assert.ok(existsSync(path.join(prefix,'versions/1.0.0/bin/axiomcli')));
    result=install(installer('1.0.2','broken'));assert.notEqual(result.status,0);assert.equal(run('axiomcli'),'axiomcli 1.0.1');assert.equal(run('axiom-proxy'),'proxy 1.0.1');
    result=install(installer('1.0.3'));assert.equal(result.status,0,result.stderr);
    // Reinstalling the same bytes is idempotent.
    assert.equal(install(path.join(directory,'1.0.3.sh')).status,0);
    assert.notEqual(install(installer('1.0.3','1.0.3','# changed bytes\n')).status,0);
    assert.equal(run('axiomcli'),'axiomcli 1.0.3');
    const remove=spawnSync('/bin/sh',[path.join(prefix,'current/uninstall.sh'),'--yes'],{env,encoding:'utf8'});
    assert.equal(remove.status,0,remove.stderr);assert.equal(existsSync(prefix),false);assert.equal(readFileSync(account,'utf8'),'preserved account data');
  }finally{rmSync(directory,{recursive:true,force:true});}
});

import assert from 'node:assert/strict';
import test from 'node:test';
import {setupFixture} from './setup-fixture.mjs';
import {parseSetup,signSetup,verifySetup} from './windows-setup.mjs';

test('setup signature covers identity, publisher and final artifact bytes',()=>{
  const {metadata,pem,key}=setupFixture();
  const signed=signSetup(metadata,pem,key);
  assert.deepEqual(verifySetup(signed,key),signed);
  assert.deepEqual(verifySetup(signed,`${'c'.repeat(64)},${key}`),signed);
  assert.throws(()=>verifySetup(signed,''),/Untrusted/);
  assert.throws(()=>signSetup(metadata,pem,'c'.repeat(64)),/not trusted/);
  assert.throws(()=>verifySetup(metadata,key),/must be signed/);
  for(const change of [value=>value.sequence++,value=>value.publisher='Someone else',value=>value.artifact.sha256='d'.repeat(64),value=>value.artifact.bytes++]) {
    const altered=structuredClone(signed);change(altered);assert.throws(()=>verifySetup(altered,key));
  }
});
test('setup metadata cannot redirect to other hosts, name mutable files, or add app targets',()=>{
  const {metadata}=setupFixture();
  for(const change of [value=>value.artifact.url='https://evil.example/Axiom.exe',value=>value.artifact.githubUrl='https://github.com/another/repository/file.exe',value=>value.artifact.name='../Axiom.exe',value=>value.artifact.bytes=51*1024*1024,value=>value.artifact.bytes=0,value=>value.revision='main',value=>value.downloads=[],value=>value.artifact.arch='universal',value=>value.sequence=NaN,value=>value.version='01.0.0']) {
    const altered=structuredClone(metadata);change(altered);assert.throws(()=>parseSetup(altered));
  }
});

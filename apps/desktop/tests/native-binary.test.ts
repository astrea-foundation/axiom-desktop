import assert from 'node:assert/strict';
import {mkdtempSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {createRequire} from 'node:module';
import test from 'node:test';
const validate = createRequire(import.meta.url)('../../../scripts/native-binary.cjs');

test('universal Mach-O requires intact, distinct x64 and ARM64 slices', () => {
  const dir=mkdtempSync(path.join(tmpdir(),'axiom-macho-')), file=path.join(dir,'binary');
  try {
    for (const wide of [false,true]) {
      const bytes=Buffer.alloc(256), entrySize=wide ? 32 : 20;
      bytes.writeUInt32BE(wide ? 0xcafebabf : 0xcafebabe,0);bytes.writeUInt32BE(2,4);
      for (const [i,cpu] of [0x01000007,0x0100000c].entries()) {
        const entry=8+i*entrySize, offset=128+i*64;
        bytes.writeUInt32BE(cpu,entry);
        if (wide) { bytes.writeBigUInt64BE(BigInt(offset),entry+8);bytes.writeBigUInt64BE(64n,entry+16); }
        else { bytes.writeUInt32BE(offset,entry+8);bytes.writeUInt32BE(64,entry+12); }
        bytes.writeUInt32LE(0xfeedfacf,offset);bytes.writeUInt32LE(cpu,offset+4);
      }
      writeFileSync(file,bytes);validate(file,'mac','universal');
      assert.throws(()=>validate(file,'mac','x64'));
      assert.throws(()=>validate(file,'win','universal'));
      writeFileSync(file,bytes.subarray(0,200));assert.throws(()=>validate(file,'mac','universal'));
      const wrong=Buffer.from(bytes);wrong.writeUInt32LE(0x01000007,196);
      writeFileSync(file,wrong);assert.throws(()=>validate(file,'mac','universal'));
      const overlap=Buffer.from(bytes);
      if (wide) overlap.writeBigUInt64BE(128n,8+entrySize+8);else overlap.writeUInt32BE(128,8+entrySize+8);
      writeFileSync(file,overlap);assert.throws(()=>validate(file,'mac','universal'));
    }
  } finally { rmSync(dir,{recursive:true,force:true}); }
});

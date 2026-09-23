const {readFileSync} = require('node:fs');
module.exports = function assertArchitecture(file, platform, arch) {
  const binary = readFileSync(file), arm = arch === 'arm64';
  let valid = false;
  if (binary.length >= 64 && ['x64','arm64','universal'].includes(arch) && (arch !== 'universal' || platform === 'mac')) {
    if (platform === 'linux') valid = binary.subarray(0,6).toString('hex') === '7f454c460201' && binary.readUInt16LE(18) === (arm ? 183 : 62);
    else if (platform === 'mac') {
      if (arch !== 'universal') valid = binary.readUInt32LE(0) === 0xfeedfacf && binary.readUInt32LE(4) === (arm ? 0x0100000c : 0x01000007);
      else {
        const magic = binary.readUInt32BE(0), wide = magic === 0xcafebabf;
        const entrySize = wide ? 32 : 20, count = binary.readUInt32BE(4);
        if ([0xcafebabe, 0xcafebabf].includes(magic) && count === 2 && binary.length >= 8 + count * entrySize) {
          const slices = [];
          for (let i=0; i<count; i++) {
            const entry = 8 + i * entrySize, cpu = binary.readUInt32BE(entry);
            const offset = wide ? Number(binary.readBigUInt64BE(entry+8)) : binary.readUInt32BE(entry+8);
            const size = wide ? Number(binary.readBigUInt64BE(entry+16)) : binary.readUInt32BE(entry+12);
            if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(size) || offset < 8 + count * entrySize || size < 32 || offset > binary.length - size ||
                binary.readUInt32LE(offset) !== 0xfeedfacf || binary.readUInt32LE(offset+4) !== cpu) break;
            slices.push({cpu, offset, end:offset+size});
          }
          slices.sort((a,b)=>a.offset-b.offset);
          valid = slices.length === 2 && slices[0].end <= slices[1].offset &&
            [0x01000007,0x0100000c].every(cpu=>slices.some(slice=>slice.cpu === cpu));
        }
      }
    }
    else if (platform === 'win') {
      const pe = binary.readUInt32LE(0x3c);
      valid = binary.readUInt16LE(0) === 0x5a4d && pe <= binary.length - 6 && binary.readUInt32LE(pe) === 0x4550 && binary.readUInt16LE(pe+4) === (arm ? 0xaa64 : 0x8664);
    }
  }
  if (!valid) throw new Error(`Native binary architecture does not match ${platform}/${arch}: ${file}`);
};

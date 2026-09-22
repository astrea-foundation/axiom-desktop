const {readFileSync} = require('node:fs');
module.exports = function assertArchitecture(file, platform, arch) {
  const binary = readFileSync(file), arm = arch === 'arm64';
  let valid = false;
  if (binary.length >= 64) {
    if (platform === 'linux') valid = binary.subarray(0,6).toString('hex') === '7f454c460201' && binary.readUInt16LE(18) === (arm ? 183 : 62);
    else if (platform === 'mac') valid = binary.readUInt32LE(0) === 0xfeedfacf && binary.readUInt32LE(4) === (arm ? 0x0100000c : 0x01000007);
    else if (platform === 'win') {
      const pe = binary.readUInt32LE(0x3c);
      valid = binary.readUInt16LE(0) === 0x5a4d && pe <= binary.length - 6 && binary.readUInt32LE(pe) === 0x4550 && binary.readUInt16LE(pe+4) === (arm ? 0xaa64 : 0x8664);
    }
  }
  if (!valid) throw new Error(`Native binary architecture does not match ${platform}/${arch}: ${file}`);
};

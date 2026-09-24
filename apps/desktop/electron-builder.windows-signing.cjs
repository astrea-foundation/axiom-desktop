const { signingSettings } = require('./scripts/windows-signing.cjs');

module.exports = {
  extends: './electron-builder.yml',
  forceCodeSigning: true,
  win: {
    signExts: ['.dll', '.node', '.ps1'],
    signtoolOptions: {
      sign: require.resolve('./scripts/windows-signing.cjs'),
      signingHashAlgorithms: ['sha256'],
      publisherName: signingSettings().PUBLISHER,
    },
  },
};

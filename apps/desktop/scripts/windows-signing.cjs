const path = require('node:path');
const { execFileSync } = require('node:child_process');

function signingSettings(env = process.env) {
  const settings = {};
  for (const key of ['ENDPOINT', 'ACCOUNT', 'PROFILE', 'PUBLISHER']) {
    const value = env[`AXIOM_SIGNING_${key}`]?.trim();
    if (!value || /[\r\n]/.test(value)) throw new Error(`AXIOM_SIGNING_${key} is required`);
    settings[key] = value;
  }
  if (!/^https:\/\/[a-z0-9]+\.codesigning\.azure\.net\/?$/.test(settings.ENDPOINT)) {
    throw new Error('Signing endpoint must be a regional Microsoft Artifact Signing HTTPS endpoint');
  }
  for (const key of ['ACCOUNT', 'PROFILE']) {
    if (!/^[a-zA-Z][a-zA-Z0-9-]{3,98}[a-zA-Z0-9]$/.test(settings[key]) || settings[key].includes('--')) {
      throw new Error(`Invalid signing ${key.toLowerCase()} name`);
    }
  }
  return settings;
}

function sign(configuration) {
  signingSettings();
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('Artifact Signing must run on an x64 Windows signing host (including for ARM64 payloads)');
  }
  if (configuration.hash !== 'sha256' || configuration.isNest) throw new Error('Only a single SHA256 signature is supported');
  execFileSync('pwsh', ['-NoProfile', '-NonInteractive', '-File', path.join(__dirname, 'sign-windows.ps1'), '-FilePath', configuration.path], { stdio: 'inherit' });
}

module.exports = { signingSettings, sign };

module.exports = function unsignedEnvironment(source) {
  const env = { ...source, CSC_IDENTITY_AUTO_DISCOVERY: 'false' };
  for (const name of Object.keys(env)) {
    if ((name.startsWith('CSC_') && name !== 'CSC_IDENTITY_AUTO_DISCOVERY') ||
        name.startsWith('WIN_CSC_') || name.startsWith('APPLE_') ||
        name.startsWith('AZURE_') || name.startsWith('AXIOM_SIGNING_')) delete env[name];
  }
  return env;
};

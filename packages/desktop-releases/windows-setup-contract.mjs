import {VERSION_PATTERN} from './manifest.mjs';

function exact(value, fields) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).sort().join(',') !== [...fields].sort().join(',')) throw new Error('Invalid Windows setup metadata');
}
export function parseSetup(value) {
  exact(value, ['schemaVersion','version','releaseVersion','revision','sequence','publisher','signing','artifact',...(value?.signature ? ['signature'] : [])]);
  if (value.schemaVersion !== 1 || !VERSION_PATTERN.test(value.version) || !VERSION_PATTERN.test(value.releaseVersion) || !/^[a-f0-9]{40}$/.test(value.revision) || !Number.isSafeInteger(value.sequence) || value.sequence < 1 || !['signed','unsigned'].includes(value.signing) || typeof value.publisher !== 'string' || !value.publisher.trim() || value.publisher.length > 120 || /[\r\n]/.test(value.publisher)) throw new Error('Invalid Windows setup identity');
  const file = value.artifact;
  exact(file,['name','bytes','sha256','url','githubUrl']);
  const name = `AxiomSetup-${value.version}-${value.revision.slice(0,12)}.exe`;
  if (file.name !== name || !Number.isSafeInteger(file.bytes) || file.bytes < 1 || file.bytes > 50*1024*1024 || !/^[a-f0-9]{64}$/.test(file.sha256) || file.url !== `https://cdn.axiom.stream/downloads/setup/${value.revision}/${name}` || file.githubUrl !== `https://github.com/astrea-foundation/axiom-releases/releases/download/v${value.releaseVersion}/${name}`) throw new Error('Invalid Windows setup artifact');
  if (value.signing === 'signed') {
    exact(value.signature,['keyId','value']);
    if (!/^[a-f0-9]{16}$/.test(value.signature.keyId) || typeof value.signature.value !== 'string' || !/^[A-Za-z0-9+/]{86}==$/.test(value.signature.value) || btoa(atob(value.signature.value)) !== value.signature.value) throw new Error('Invalid Windows setup signature');
  } else if (value.signature) throw new Error('Unsigned setup has a signature');
  return structuredClone(value);
}

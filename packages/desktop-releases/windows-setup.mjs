// Setup is a separate artifact, never a ninth app-update target.
import {createPublicKey, sign, verify} from 'node:crypto';
import {canonicalJson} from './manifest.mjs';
import {parseSetup} from './windows-setup-contract.mjs';
export {parseSetup} from './windows-setup-contract.mjs';
import {keyId, publicKeyHex} from './signing.mjs';

const unsigned = value => { const result = {...value}; delete result.signature; return result; };
export function signSetup(metadata, privateKey, trustedKeys) {
  const key = publicKeyHex(privateKey);
  if (!trustedKeys.split(',').map(key=>key.trim()).includes(key)) throw new Error('Setup signing key is not trusted by this release');
  const value = {...unsigned(parseSetup(metadata)),signing:'signed'};
  return parseSetup({...value,signature:{keyId:keyId(key),value:sign(null,Buffer.from(canonicalJson(value)),privateKey).toString('base64')}});
}
export function verifySetup(metadata, trustedKeys) {
  const value = parseSetup(metadata);
  if (value.signing !== 'signed') throw new Error('Windows setup must be signed');
  const key = trustedKeys.split(',').map(key=>key.trim()).find(key=>/^[a-f0-9]{64}$/.test(key) && keyId(key) === value.signature.keyId);
  if (!key) throw new Error('Untrusted Windows setup signing key');
  const publicKey = createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(key,'hex')]),format:'der',type:'spki'});
  if (!verify(null,Buffer.from(canonicalJson(unsigned(value))),publicKey,Buffer.from(value.signature.value,'base64'))) throw new Error('Invalid Windows setup signature');
  return value;
}

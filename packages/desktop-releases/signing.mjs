import { createHash, createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';
import { canonicalJson, parseRelease, unsignedManifest } from './manifest.mjs';

export function publicKeyHex(privateKeyPem) {
  const key = createPublicKey(createPrivateKey(privateKeyPem));
  if (key.asymmetricKeyType !== 'ed25519') throw new Error('Update key must be Ed25519');
  return key.export({format: 'der', type: 'spki'}).subarray(-32).toString('hex');
}
export function keyId(hex) { return createHash('sha256').update(Buffer.from(hex, 'hex')).digest('hex').slice(0, 16); }
export function signRelease(manifest, privateKeyPem, expectedPublicKey) {
  if (!/^[a-f0-9]{64}$/.test(expectedPublicKey ?? '') || publicKeyHex(privateKeyPem) !== expectedPublicKey) throw new Error('Update signing identity does not match the compiled trust key');
  const value = {...unsignedManifest(manifest), signing: 'signed'};
  return parseRelease({...value, signature: { keyId: keyId(expectedPublicKey), value: sign(null, Buffer.from(canonicalJson(value)), privateKeyPem).toString('base64') }});
}
export function verifyRelease(manifest, publicKey) {
  const value = parseRelease(manifest);
  if (value.signing !== 'signed' || !/^[a-f0-9]{64}$/.test(publicKey ?? '') || value.signature.keyId !== keyId(publicKey)) throw new Error('Untrusted update release');
  const key = createPublicKey({key: Buffer.concat([Buffer.from('302a300506032b6570032100','hex'), Buffer.from(publicKey,'hex')]), format: 'der', type: 'spki'});
  if (!verify(null, Buffer.from(canonicalJson(unsignedManifest(value))), key, Buffer.from(value.signature.value,'base64'))) throw new Error('Invalid update signature');
  return value;
}

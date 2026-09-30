// Test identities only; generated independently for each test invocation.
import {generateKeyPairSync} from 'node:crypto';
import {publicKeyHex} from './signing.mjs';

export function setupFixture(release = {version:'0.1.10',revision:'a'.repeat(40),sequence:123}) {
  const {privateKey}=generateKeyPairSync('ed25519');
  const pem=privateKey.export({format:'pem',type:'pkcs8'}), key=publicKeyHex(pem);
  const name=`AxiomSetup-1.0.0-${release.revision.slice(0,12)}.exe`;
  const metadata={schemaVersion:1,version:'1.0.0',releaseVersion:release.version,revision:release.revision,sequence:release.sequence,publisher:'Astrea Labs, Inc.',signing:'unsigned',
    artifact:{name,bytes:4,sha256:'b'.repeat(64),url:`https://cdn.axiom.stream/downloads/setup/${release.revision}/${name}`,githubUrl:`https://github.com/astrea-foundation/axiom-releases/releases/download/v${release.version}/${name}`}};
  return {metadata,pem,key};
}

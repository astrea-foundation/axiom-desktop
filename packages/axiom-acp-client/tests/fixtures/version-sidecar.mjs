import { createInterface } from 'node:readline';

const version = process.argv[2];
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.method !== 'initialize') throw new Error('Version must be accepted before bootstrap');
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
    protocolVersion: 1,
    ...(version === 'missing' ? {} : { agentInfo: { name: 'axiomcli', version } }),
    agentCapabilities: { _meta: { axiom: { protocolVersion: '0.2', features: {
      desktopChat: 1, threadCatalog: 1, timeline: 2, modelCatalog: 1,
      profilePreferences: 1, collections: 1, account: 2, billing: 3,
      securityEvidence: 4, webConsent: 1,
    } } } },
  } }) + '\n');
}

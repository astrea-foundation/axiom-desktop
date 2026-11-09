import { execFileSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const service = 'stream.axiom.axiomcli';
const account = 'native-refresh-token-v1-' + createHash('sha256').update('https://api.axiom.stream').digest('base64url');
const quoted = value => {
  if (/[\r\n\0]/.test(value)) throw new Error('Invalid native QA argument');
  return '"' + value.replaceAll('\\', '\\\\').replaceAll('"', '\\"') + '"';
};

export function nativeQaFixture(source) {
  if (source.GITHUB_ACTIONS !== 'true' || source.RUNNER_ENVIRONMENT !== 'github-hosted'
      || source.RUNNER_OS !== 'macOS' || !path.isAbsolute(source.RUNNER_TEMP ?? '')) {
    throw new Error('Native release credentials can only be prepared on a disposable GitHub macOS runner');
  }
  let fixture;
  try { fixture = JSON.parse(source.AXIOM_INSTALLED_NATIVE_QA ?? ''); }
  catch { throw new Error('The protected native-session QA credential is required; an automation API key cannot sign into ACP'); }
  if (!/^\d+\.\d+\.\d+$/.test(fixture.version ?? '') || !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/.test(fixture.accountId ?? '')
      || !/^[A-Za-z0-9_-]{32,128}$/.test(fixture.clientId ?? '')
      || !/^axr_[A-Za-z0-9_-]{32,128}$/.test(fixture.refreshToken ?? '')
      || !/^[0-9a-f]{32}$/.test(fixture.sessionId ?? '')) {
    throw new Error('Invalid native-session release QA credential');
  }
  return fixture;
}

// The production account is origin scoped, so use a separate keychain only on
// this disposable runner. Both products reuse the rotating entry; never seed
// the original refresh token again after the first installed process uses it.
export function prepareNativeReleaseQa(source, environment, version, run = (args, input) => execFileSync('/usr/bin/security', args, { input, stdio: ['pipe', 'pipe', 'pipe'] })) {
  const fixture = nativeQaFixture(source);
  if (fixture.version !== version) throw new Error('The native QA credential belongs to a different release');
  const marker = path.join(source.RUNNER_TEMP, 'axiom-installed-native-qa.json');
  const keychain = path.join(source.RUNNER_TEMP, 'axiom-installed-native-qa.keychain-db');
  const identity = path.join(environment.XDG_DATA_HOME, 'axiom/shared/identity/client-id');
  if (!path.isAbsolute(environment.XDG_DATA_HOME) || !identity.startsWith(path.resolve(source.RUNNER_TEMP) + path.sep)) {
    throw new Error('Native release QA profile must stay inside the disposable runner temporary directory');
  }
  if (existsSync(marker)) {
    const previous = JSON.parse(readFileSync(marker, 'utf8'));
    if (previous.clientId !== fixture.clientId || previous.accountId !== fixture.accountId || previous.sessionId !== fixture.sessionId) {
      throw new Error('A different native QA session already owns this runner');
    }
  } else {
    if (existsSync(keychain)) throw new Error('Reconcile the existing native QA keychain before restarting acceptance');
    const password = randomBytes(32).toString('hex');
    const commands = [
      `create-keychain -p ${quoted(password)} ${quoted(keychain)}`,
      `set-keychain-settings -lut 10800 ${quoted(keychain)}`,
      `unlock-keychain -p ${quoted(password)} ${quoted(keychain)}`,
      `add-generic-password -A -a ${quoted(account)} -s ${quoted(service)} -w ${quoted(fixture.refreshToken)} ${quoted(keychain)}`,
      `list-keychains -d user -s ${quoted(keychain)}`,
    ];
    try {
      // Secrets travel on stdin, never argv or release logs.
      for (const command of commands) run(['-i'], command + '\n');
      run(['find-generic-password', '-a', account, '-s', service, keychain]);
    } catch { throw new Error('Could not prepare the isolated native-session QA keychain'); }
    writeFileSync(marker, JSON.stringify({ clientId: fixture.clientId, accountId: fixture.accountId, sessionId: fixture.sessionId }) + '\n', { mode: 0o600 });
  }
  mkdirSync(path.dirname(identity), { recursive: true, mode: 0o700 });
  if (existsSync(identity) && readFileSync(identity, 'utf8').trim() !== fixture.clientId) throw new Error('The native QA installation identity changed');
  writeFileSync(identity, fixture.clientId + '\n', { mode: 0o600 });
  return { accountId: fixture.accountId, sessionId: fixture.sessionId };
}

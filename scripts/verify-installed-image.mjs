import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { prepareNativeReleaseQa } from './native-release-qa.mjs';

const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

// Deliberately do not inherit provider, verifier, profile, or test overrides.
export function installedEnvironment(source, root) {
  const environment = {};
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'LANG', 'TMPDIR', 'SYSTEMROOT']) {
    if (source[key]) environment[key] = source[key];
  }
  return { ...environment, AXIOM_MODEL: 'direct:qwen3.8-27b',
    XDG_CONFIG_HOME: path.join(root, 'config'), XDG_DATA_HOME: path.join(root, 'data'),
    XDG_CACHE_HOME: path.join(root, 'cache') };
}

export function verifiedImageUsage(requests) {
  const images = requests.filter(request => /^\d+$/.test(request.imagePixels ?? '') && BigInt(request.imagePixels) > 0n);
  if (images.length !== 1) throw new Error('Expected exactly one image request in installed-session usage');
  const usage = images[0];
  if (usage.providerId !== 'axiom-direct' || usage.state !== 'completed' || usage.completeness !== 'final'
      || usage.responseVerified !== true || usage.settled !== true
      || !/^\d+$/.test(usage.costMicrousd ?? '')) {
    throw new Error('Installed image request must have authenticated, final, settled usage');
  }
  return usage;
}

export function verifyImageTools(calls) {
  const generation = calls.filter(call => call.title.endsWith(' · generate'));
  const viewing = calls.filter(call => call.title.endsWith(' · view_image'));
  if (generation.length !== 1 || generation[0].status !== 'completed'
      || !viewing.some(call => call.status === 'completed')) {
    throw new Error('Installed session must successfully complete one generation and image viewing');
  }
}

export async function verifyInstalledImage(binary, installer, output, version, source = process.env) {
  if (!/^\d+\.\d+\.\d+$/.test(version ?? '')) throw new Error('A stable release version is required');
  output = path.resolve(output);
  mkdirSync(output, { recursive: true, mode: 0o700 });
  const environment = installedEnvironment(source, output);
  const nativeQa = prepareNativeReleaseQa(source, environment, version);
  const installedVersion = execFileSync(binary, ['--version'], { env: environment, encoding: 'utf8', timeout: 30000 }).trim();
  if (installedVersion !== `axiomcli ${version}`) throw new Error('Installed CLI version does not match the release');
  const child = spawn(binary, ['acp', '--frontend', 'cli', ...(process.platform === 'darwin' ? ['--no-keychain-ui'] : [])],
    { cwd: output, env: environment, stdio: ['pipe', 'pipe', 'pipe'] });
  let buffer = '', sequence = 0, sessionId, closed = false;
  const pending = new Map(), images = new Map(), tools = new Map();
  const send = message => child.stdin.write(JSON.stringify(message) + '\n');
  function fail(error) {
    for (const item of pending.values()) { clearTimeout(item.timeout); item.reject(error); }
    pending.clear();
  }
  function findImages(value) {
    if (!value || typeof value !== 'object') return;
    if (value.type === 'image' && value.mimeType === 'image/png' && typeof value.data === 'string') {
      const bytes = Buffer.from(value.data, 'base64');
      if (bytes.length < 24 || bytes.toString('hex', 0, 8) !== '89504e470d0a1a0a') throw new Error('Invalid generated PNG');
      const hash = sha256(bytes), width = bytes.readUInt32BE(16), height = bytes.readUInt32BE(20);
      if (!width || !height || width * height > 40000000) throw new Error('Invalid generated image dimensions');
      if (!images.has(hash)) {
        const name = `image-${hash.slice(0, 12)}.png`;
        writeFileSync(path.join(output, name), bytes, { mode: 0o600 });
        images.set(hash, { name, sha256: hash, bytes: bytes.length, width, height });
      }
    }
    for (const item of Object.values(value)) {
      if (Array.isArray(item)) item.forEach(findImages);
      else if (item && typeof item === 'object') findImages(item);
    }
  }
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', chunk => {
    try {
      buffer += chunk;
      if (buffer.length > 64 * 1024 * 1024) throw new Error('Installed ACP output exceeded its limit');
      for (let newline; (newline = buffer.indexOf('\n')) >= 0;) {
        const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
        if (!line.trim()) continue;
        const message = JSON.parse(line);
        if (message.method === 'session/request_permission') {
          send({ jsonrpc: '2.0', id: message.id, result: { outcome: { outcome: 'cancelled' } } });
        } else if (message.method === 'session/update' && message.params?.sessionId === sessionId) {
          const update = message.params.update;
          if (update?.sessionUpdate === 'tool_call' && typeof update.title === 'string') {
            tools.set(update.toolCallId, { id: update.toolCallId, title: update.title, status: update.status });
          } else if (update?.sessionUpdate === 'tool_call_update' && tools.has(update.toolCallId) && update.status) {
            tools.get(update.toolCallId).status = update.status;
          }
          findImages(update);
        } else if (message.id !== undefined && !message.method && pending.has(message.id)) {
          const entry = pending.get(message.id); pending.delete(message.id); clearTimeout(entry.timeout);
          if (message.error) entry.reject(new Error(`Installed ACP request failed: ${entry.method} (${message.error.code})`));
          else entry.resolve(message.result);
        }
      }
    } catch (error) { fail(error); child.kill('SIGTERM'); }
  });
  // Do not put credentials or provider error bodies into release logs.
  child.stderr.resume();
  child.stdin.on('error', fail);
  child.on('error', fail);
  child.once('exit', () => { closed = true; fail(new Error('Installed ACP process exited before verification completed')); });
  function request(method, params, timeoutMs = 30000) {
    const id = ++sequence;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { pending.delete(id); reject(new Error(`Installed ACP timeout: ${method}`)); }, timeoutMs);
      pending.set(id, { resolve, reject, timeout, method }); send({ jsonrpc: '2.0', id, method, params });
    });
  }
  try {
    const initialized = await request('initialize', { protocolVersion: 1,
      clientInfo: { name: 'signed-installation-image-acceptance', version: '1' },
      clientCapabilities: { _meta: { axiom: { protocolVersion: '0.2', features: {
        modelCatalog: 1, account: 2, imageGeneration: 4, activity: 1, attachments: 2,
        desktopChat: 1, threadCatalog: 1, timeline: 2,
      } } } } });
    const imageGenerationVersion = initialized.agentCapabilities?._meta?.axiom?.features?.imageGeneration;
    if (imageGenerationVersion !== 4) throw new Error('Installed image tools were not registered from the bundled configuration');
    const account = await request('_axiom/account/status', {});
    if (account.status.state !== 'valid') throw new Error('Installed production account is unavailable');
    if (account.status.account?.id !== nativeQa.accountId || account.status.session?.credentialStore !== 'system credential store') {
      throw new Error('Installed acceptance must use its own native-session account and OS credential store');
    }
    const session = await request('session/new', { cwd: output, mcpServers: [] });
    sessionId = session.sessionId;
    const model = session.configOptions?.find(option => option.category === 'model' || option.id === 'model')?.currentValue;
    if (model !== environment.AXIOM_MODEL) throw new Error('Installed session selected an unexpected model');
    const result = await request('session/prompt', { sessionId, prompt: [{ type: 'text',
      text: 'Generate exactly one image: a single flat blue circle centered on a plain white square background. Call generate once with a 1:1 aspect ratio. Then call view_image to inspect that generated image. Keep your final reply brief.' }] }, 300000);
    const page = await request('_axiom/thread/timeline', { threadId: sessionId, limit: 500 });
    findImages(page.items);
    if (result.stopReason !== 'end_turn' || page.thread.lifecycle !== 'ready' || page.activeTurnId) throw new Error('Installed image turn did not finish ready');
    if (images.size !== 1) {
      throw new Error('Installed session did not generate and view exactly one image');
    }
    const toolStates = [...tools.values()];
    verifyImageTools(toolStates);
    const usage = verifiedImageUsage(page.requestUsage ?? []);
    const receipt = { version, revision: source.AXIOM_QUALIFIED_NATIVE_REVISION ?? source.GITHUB_SHA ?? null,
      acceptanceRevision: source.GITHUB_SHA ?? null, installedVersion,
      installer: { name: path.basename(installer), sha256: sha256(readFileSync(installer)) },
      binary: { path: binary, sha256: sha256(readFileSync(binary)) },
      configurationSource: 'installed-bundle', credentialSource: 'native-session-keychain', model, imageGenerationVersion, sessionId,
      lifecycle: page.thread.lifecycle, stopReason: result.stopReason, tools: toolStates.map(call => call.title),
      toolStates, images: [...images.values()], usage };
    writeFileSync(path.join(output, 'receipt.json'), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
    console.log(JSON.stringify({ verified: true, version, installer: receipt.installer, images: receipt.images, settled: usage.settled, responseVerified: usage.responseVerified }));
    return receipt;
  } finally {
    child.stdin.end();
    if (!closed) {
      const terminate = setTimeout(() => { if (!closed) child.kill('SIGTERM'); }, 2000);
      terminate.unref();
    }
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { await verifyInstalledImage(...process.argv.slice(2)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}

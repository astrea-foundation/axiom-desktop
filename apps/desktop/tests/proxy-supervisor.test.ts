import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createServer, connect } from "node:net";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { ProxySupervisor } from "../src/main/proxy-supervisor";

async function freePort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as { port: number }).port;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return port;
}

function childFor(port: number, mode = "ready"): ChildProcessWithoutNullStreams {
  return spawn(process.execPath, ["-e", `
    const server = require('node:net').createServer(socket => socket.end());
    process.stdin.resume();
    process.stdin.on('end', () => process.exit(0));
    server.listen(${port}, '127.0.0.1', () => {
      const ready = {event:'ready',address:'127.0.0.1:${port}',openai_base_url:'http://127.0.0.1:${port}/v1',security:'attestation_per_request'};
      const mode = ${JSON.stringify(mode)};
      if (mode === 'invalid') ready.address = '0.0.0.0:${port}';
      if (mode !== 'silent') process.stdout.write(JSON.stringify(ready)+'\\n');
      if (mode === 'metadata') {
        process.stderr.write('raw-secret-must-not-reach-renderer\\n');
        process.stderr.write(JSON.stringify({event:'security_verified',request_id:'request-1',model_id:'model',provider_id:'near',e2ee_protocol:'test-e2ee',verified_at_unix_seconds:1700000000,model_key_fingerprint:'ab'.repeat(32),extra:'secret'})+'\\n');
        process.stderr.write(JSON.stringify({event:'request_terminal',request_id:'request-1',terminal:'completed',total_tokens:7})+'\\n');
        process.stderr.write(JSON.stringify({event:'request_failed',failure_kind:'Transient',safe_detail:'secret'})+'\\n');
      }
    });
  `], { stdio: "pipe" });
}

async function waitFor(check: () => boolean) {
  for (let i = 0; i < 100; i++) { if (check()) return; await delay(10); }
  assert.ok(check(), "condition did not become true");
}

test("proxy starts a real owned listener, reports metadata, rotates tokens and stops it", async () => {
  const launched: ChildProcessWithoutNullStreams[] = [];
  const suppliedTokens: string[] = [];
  const proxy = new ProxySupervisor(async (port, account, token) => {
    assert.equal(account, "account-a"); suppliedTokens.push(token);
    const child = childFor(port, "metadata"); launched.push(child); return child;
  }, () => {});
  try {
    assert.equal(proxy.snapshot().status, "stopped");
    assert.throws(() => proxy.start(8484, "account-a", "runtime"), /account/);
    proxy.setAccount("account-a", "runtime");
    const port = await freePort();
    await proxy.start(port, "account-a", "runtime");
    assert.equal(proxy.snapshot().baseUrl, `http://127.0.0.1:${port}/v1`);
    await new Promise<void>((resolve, reject) => { const socket = connect(port, "127.0.0.1", () => { socket.destroy(); resolve(); }); socket.once("error", reject); });
    await waitFor(() => proxy.snapshot().completedRequests === 1);
    assert.equal(proxy.snapshot().totalTokens, 7);
    assert.equal(proxy.snapshot().evidence?.responseVerified, true);
    assert.equal(proxy.snapshot().errors[0]?.kind, "Transient");
    const token = proxy.copyToken("account-a", "runtime");
    assert.match(token, /^[a-f0-9]{64}$/);
    assert.doesNotMatch(JSON.stringify(proxy.snapshot()), /secret/);
    assert.ok(!JSON.stringify(proxy.snapshot()).includes(token));
    await proxy.start(await freePort(), "account-a", "runtime");
    assert.notEqual(proxy.copyToken("account-a", "runtime"), token);
    assert.equal(launched[0]!.exitCode, 0);
    await proxy.stop();
    assert.equal(proxy.snapshot().status, "stopped");
    assert.throws(() => proxy.copyToken("account-a", "runtime"), /Start/);
    assert.equal(launched[1]!.exitCode, 0);
    assert.equal(suppliedTokens.length, 2);
  } finally { await proxy.dispose(); }
});

test("account changes revoke the token, stop the process and clear old metadata", async () => {
  let child: ChildProcessWithoutNullStreams | undefined;
  const proxy = new ProxySupervisor(async (port) => child = childFor(port, "metadata"), () => {});
  try {
    proxy.setAccount("a", "runtime");
    await proxy.start(await freePort(), "a", "runtime");
    await waitFor(() => proxy.snapshot().totalTokens === 7);
    proxy.setAccount("b", "runtime");
    assert.equal(proxy.snapshot().accountId, "b");
    assert.equal(proxy.snapshot().evidence, null);
    assert.equal(proxy.snapshot().totalTokens, 0);
    assert.throws(() => proxy.copyToken("a", "runtime"), /account/);
    await waitFor(() => child?.exitCode === 0);
    assert.equal(proxy.snapshot().accountId, "b");
    assert.equal(proxy.snapshot().status, "stopped");
  } finally { await proxy.dispose(); }
});

for (const mode of ["invalid", "silent"]) {
  test(`proxy fails closed on ${mode} readiness`, async () => {
    const proxy = new ProxySupervisor(async (port) => childFor(port, mode), () => {}, 150);
    try {
      proxy.setAccount("a", "runtime");
      await assert.rejects(proxy.start(await freePort(), "a", "runtime"));
      assert.equal(proxy.snapshot().status, "failed");
      assert.equal(proxy.snapshot().baseUrl, null);
      assert.throws(() => proxy.copyToken("a", "runtime"), /Start/);
    } finally { await proxy.dispose(); }
  });
}

test("stop during asynchronous launch cannot resurrect the proxy", async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let child: ChildProcessWithoutNullStreams | undefined;
  const proxy = new ProxySupervisor(async (port) => { await gate; return child = childFor(port); }, () => {});
  try {
    proxy.setAccount("a", "runtime");
    const starting = proxy.start(await freePort(), "a", "runtime");
    const rejected = assert.rejects(starting, /cancelled/);
    await waitFor(() => proxy.snapshot().status === "starting");
    await proxy.stop();
    release();
    await rejected;
    assert.equal(proxy.snapshot().status, "stopped");
    assert.equal(child?.exitCode, 0);
  } finally { release(); await proxy.dispose(); }
});

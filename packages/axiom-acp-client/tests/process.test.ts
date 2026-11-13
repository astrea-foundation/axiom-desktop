import assert from "node:assert/strict";
import { once } from "node:events";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  AcpProcess,
  DESKTOP_SIDECAR_INHERITED_ENV,
  ProtocolError,
  createChildEnvironment,
  type IncomingRequest,
  type RpcNotification,
} from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));

test("sidecar environment overrides endpoints and removes credentials case-insensitively", () => {
  const environment = createChildEnvironment(
    {
      AXIOM_API_KEY: "axm_inherited",
      axiom_api_key: "axm_mixed_case",
      AXIOM_BASE_URL: "https://wrong.example",
      KEEP: "yes",
    },
    { AXIOM_BASE_URL: "https://api.axiom.stream" },
    ["AXIOM_API_KEY"],
  );
  assert.equal(environment.AXIOM_API_KEY, undefined);
  assert.equal(environment.axiom_api_key, undefined);
  assert.equal(environment.AXIOM_BASE_URL, "https://api.axiom.stream");
  assert.equal(environment.KEEP, "yes");
});

test("Desktop sidecar allowlist excludes inherited application secrets", () => {
  const environment = createChildEnvironment(
    {
      Path: "C:\\Windows\\System32",
      HOME: "/home/ada",
      DBUS_SESSION_BUS_ADDRESS: "unix:path=/run/user/1000/bus",
      AXIOM_API_KEY: "axm_secret",
      axiom_auth_url: "https://attacker.example",
      DATABASE_URL: "postgres://secret",
      WALLET_SEED: "zcash-secret",
      OPENAI_API_KEY: "provider-secret",
    },
    {
      AXIOM_AUTH_URL: "https://auth.axiom.stream",
      AXIOM_BASE_URL: "https://api.axiom.stream",
    },
    ["AXIOM_API_KEY"],
    DESKTOP_SIDECAR_INHERITED_ENV,
  );

  assert.equal(environment.Path, "C:\\Windows\\System32");
  assert.equal(environment.HOME, "/home/ada");
  assert.equal(environment.DBUS_SESSION_BUS_ADDRESS, "unix:path=/run/user/1000/bus");
  assert.equal(environment.AXIOM_AUTH_URL, "https://auth.axiom.stream");
  assert.equal(environment.AXIOM_BASE_URL, "https://api.axiom.stream");
  for (const name of [
    "AXIOM_API_KEY",
    "axiom_api_key",
    "axiom_auth_url",
    "DATABASE_URL",
    "WALLET_SEED",
    "OPENAI_API_KEY",
  ]) {
    assert.equal(environment[name], undefined, `${name} leaked into the sidecar`);
  }
});

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

async function waitForExit(pid: number): Promise<boolean> {
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline) {
    if (!processExists(pid)) return true;
    await new Promise((resolveWait) => setTimeout(resolveWait, 25));
  }
  return !processExists(pid);
}

function announcedGrandchild(sidecar: AcpProcess): Promise<number> {
  return new Promise<number>((resolvePid, reject) => {
    const timer = setTimeout(() => reject(new Error("fixture did not announce its grandchild")), 2_000);
    sidecar.on("notification", (notification: RpcNotification) => {
      if (notification.method !== "fixture/grandchild") return;
      const pid = (notification.params as { pid?: unknown } | undefined)?.pid;
      if (typeof pid !== "number" || !Number.isSafeInteger(pid) || pid <= 0) {
        clearTimeout(timer);
        reject(new Error("fixture returned an invalid grandchild PID"));
        return;
      }
      clearTimeout(timer);
      resolvePid(pid);
    });
  });
}

function fixtureReady(sidecar: AcpProcess): Promise<number> {
  return new Promise<number>((resolvePid, reject) => {
    const timer = setTimeout(() => reject(new Error("fixture did not become ready")), 2_000);
    sidecar.on("notification", (notification: RpcNotification) => {
      if (notification.method !== "fixture/ready") return;
      const pid = (notification.params as { pid?: unknown } | undefined)?.pid;
      if (typeof pid !== "number" || !Number.isSafeInteger(pid) || pid <= 0) {
        clearTimeout(timer);
        reject(new Error("fixture returned an invalid PID"));
        return;
      }
      clearTimeout(timer);
      // closeSync(0) is a no-op on Windows. Close the actual parent pipe there;
      // Unix keeps exercising EPIPE from a child that closes its read end.
      if (process.platform === "win32") sidecar.child.stdin.destroy();
      resolvePid(pid);
    });
  });
}

function closedInputSidecar(mode = "passive"): AcpProcess {
  return new AcpProcess({
    command: process.execPath,
    args: [resolve(here, "fixtures/closed-stdin-sidecar.mjs"), mode],
  });
}

test("request rejects and aborts when a live sidecar input pipe is closed", { timeout: 10_000 }, async () => {
  const sidecar = closedInputSidecar();
  try {
    const ready = fixtureReady(sidecar);
    const exited = once(sidecar, "exit");
    const pid = await ready;
    assert.equal(processExists(pid), true);

    await assert.rejects(
      sidecar.request("fixture/request", {}, 5_000),
      (error: unknown) => error instanceof ProtocolError && /write|input stream/i.test(error.message),
    );
    const [error] = await exited;
    assert.ok(error instanceof ProtocolError);
    assert.equal(await waitForExit(pid), true, "sidecar survived failed request write");
  } finally {
    await sidecar.close().catch(() => undefined);
  }
});

test("notification rejects and aborts when a live sidecar input pipe is closed", {
  timeout: 10_000,
}, async () => {
  const sidecar = closedInputSidecar();
  try {
    const ready = fixtureReady(sidecar);
    const exited = once(sidecar, "exit");
    const pid = await ready;
    assert.equal(processExists(pid), true);

    await assert.rejects(
      sidecar.notify("fixture/notification", {}),
      (error: unknown) => error instanceof ProtocolError && /write|input stream/i.test(error.message),
    );
    const [error] = await exited;
    assert.ok(error instanceof ProtocolError);
    assert.equal(await waitForExit(pid), true, "sidecar survived failed notification write");
  } finally {
    await sidecar.close().catch(() => undefined);
  }
});

test("permission response to a closed pipe during shutdown exits without an unhandled rejection", {
  timeout: 10_000,
}, async () => {
  const sidecar = closedInputSidecar("permission");
  try {
    const ready = fixtureReady(sidecar);
    const incoming = once(sidecar, "request") as Promise<[IncomingRequest]>;
    const exited = once(sidecar, "exit");
    const pid = await ready;
    const [permission] = await incoming;
    assert.equal(permission.request.method, "session/request_permission");

    permission.respond({ outcome: { outcome: "selected", optionId: "allow_once" } });
    const closing = sidecar.close();

    const [error] = await exited;
    await closing;
    assert.ok(error instanceof ProtocolError);
    assert.match(error.message, /write|input stream/i);
    assert.equal(await waitForExit(pid), true, "sidecar survived failed permission response write");
  } finally {
    await sidecar.close().catch(() => undefined);
  }
});

test("forced POSIX close kills a stubborn sidecar grandchild process group", {
  skip: process.platform === "win32",
  timeout: 15_000,
}, async () => {
  const sidecar = new AcpProcess({
    command: process.execPath,
    args: [resolve(here, "fixtures/stubborn-sidecar.mjs")],
  });
  let grandchildPid: number | undefined;
  try {
    grandchildPid = await announcedGrandchild(sidecar);
    assert.equal(processExists(grandchildPid), true);

    await sidecar.close();

    assert.equal(await waitForExit(grandchildPid), true, "grandchild survived forced group close");
  } finally {
    await sidecar.close().catch(() => undefined);
    if (grandchildPid && processExists(grandchildPid)) {
      try {
        process.kill(grandchildPid, "SIGKILL");
      } catch {
        // It exited between the liveness check and cleanup.
      }
    }
  }
});

test("unexpected POSIX leader exit still cleans its detached grandchild group", {
  skip: process.platform === "win32",
  timeout: 10_000,
}, async () => {
  const sidecar = new AcpProcess({
    command: process.execPath,
    args: [resolve(here, "fixtures/exiting-sidecar.mjs")],
  });
  const exited = once(sidecar, "exit");
  let grandchildPid: number | undefined;
  try {
    grandchildPid = await announcedGrandchild(sidecar);
    await exited;
    await sidecar.close();

    assert.equal(await waitForExit(grandchildPid), true, "grandchild survived leader exit cleanup");
  } finally {
    await sidecar.close().catch(() => undefined);
    if (grandchildPid && processExists(grandchildPid)) {
      try {
        process.kill(grandchildPid, "SIGKILL");
      } catch {
        // It exited between the liveness check and cleanup.
      }
    }
  }
});

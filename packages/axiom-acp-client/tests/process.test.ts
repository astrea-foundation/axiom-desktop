import assert from "node:assert/strict";
import { once } from "node:events";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { AcpProcess, type RpcNotification } from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));

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

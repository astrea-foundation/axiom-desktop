import { closeSync } from "node:fs";

const mode = process.argv[2] ?? "passive";

// Keep the process and stdout alive after closing the ACP input pipe. This
// deterministically exercises the parent's write-error path instead of the
// ordinary child-exit path.
// Windows libuv intentionally does not close descriptors 0-2. On Windows the
// harness closes the parent's writable pipe after the ready notification.
if (process.platform !== "win32") closeSync(0);
process.stdout.write(`${JSON.stringify({
  jsonrpc: "2.0",
  method: "fixture/ready",
  params: { pid: process.pid },
})}\n`);

if (mode === "permission") {
  process.stdout.write(`${JSON.stringify({
    jsonrpc: "2.0",
    id: 41,
    method: "session/request_permission",
    params: {
      sessionId: "fixture-session",
      options: [{ optionId: "allow_once", name: "Allow once", kind: "allow_once" }],
    },
  })}\n`);
}

setInterval(() => undefined, 1_000);

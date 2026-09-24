import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const grandchild = spawn(process.execPath, [join(here, "stubborn-grandchild.mjs")], {
  stdio: "ignore",
  shell: false,
});

process.stdout.write(`${JSON.stringify({
  jsonrpc: "2.0",
  method: "fixture/grandchild",
  params: { pid: grandchild.pid },
})}\n`, () => process.exit(0));

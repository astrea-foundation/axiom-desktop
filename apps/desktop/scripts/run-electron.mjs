import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { setLinuxDesktopIdentity } from "./desktop-identity.mjs";

const [target, ...args] = process.argv.slice(2);
const require = createRequire(import.meta.url);
const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;
setLinuxDesktopIdentity(env, process.platform);

function packageBinary(packageName) {
  const packagePath = require.resolve(`${packageName}/package.json`);
  const manifest = JSON.parse(readFileSync(packagePath, "utf8"));
  const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.[packageName];
  if (!bin) throw new Error(`${packageName} does not declare a CLI binary`);
  return join(dirname(packagePath), bin);
}

function electronBinary() {
  const packagePath = require.resolve("electron/package.json");
  const electronDir = dirname(packagePath);
  const relative = readFileSync(join(electronDir, "path.txt"), "utf8").trim();
  const binary = join(electronDir, "dist", relative);
  if (!relative || !existsSync(binary)) throw new Error("Electron binary is missing. Re-run pnpm install.");
  return binary;
}

let command;
let commandArgs;
if (target === "staging") {
  env.AXIOM_AUTH_URL = "https://auth-staging.axiom.stream";
  env.AXIOM_BASE_URL = "https://api-staging.axiom.stream";
  env.AXIOM_DESKTOP_STAGING_PROFILE = "1";
  command = process.execPath;
  commandArgs = [packageBinary("electron-vite"), ...args];
} else if (target === "vite") {
  command = process.execPath;
  commandArgs = [packageBinary("electron-vite"), ...args];
} else if (target === "electron") {
  command = electronBinary();
  commandArgs = args;
} else {
  throw new Error("usage: run-electron.mjs <vite|electron|staging> [...arguments]");
}

const child = spawn(command, commandArgs, {
  cwd: process.cwd(),
  env,
  stdio: "inherit",
  windowsHide: false,
  shell: false,
});

child.once("error", (error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
child.once("exit", (code, signal) => {
  if (signal) {
    console.error(`${target} exited after signal ${signal}`);
    process.exitCode = 1;
  } else {
    process.exitCode = code ?? 1;
  }
});

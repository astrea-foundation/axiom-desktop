import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);
const electronDir = dirname(require.resolve("electron/package.json"));
const binary = join(electronDir, "dist", "electron");

if (existsSync(binary)) {
  process.exit(0);
}

const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;

const install = spawnSync(process.execPath, [join(electronDir, "install.js")], {
  cwd: electronDir,
  stdio: "inherit",
  env,
});

if (!existsSync(binary)) {
  console.error("Electron binary is missing. Re-run pnpm install.");
  process.exit(1);
}

process.exit(install.status ?? 0);

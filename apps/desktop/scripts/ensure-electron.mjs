import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { brandDevBundle } from "./brand-dev-bundle.mjs";

const require = createRequire(import.meta.url);
const electronDir = dirname(require.resolve("electron/package.json"));

function installedBinary() {
  const pathFile = join(electronDir, "path.txt");
  if (!existsSync(pathFile)) return null;
  const relative = readFileSync(pathFile, "utf8").trim();
  if (!relative) return null;
  const binary = join(electronDir, "dist", relative);
  return existsSync(binary) ? binary : null;
}

if (installedBinary()) {
  brandDevBundle();
  process.exit(0);
}

const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;

const install = spawnSync(process.execPath, [join(electronDir, "install.js")], {
  cwd: electronDir,
  stdio: "inherit",
  env,
});

if (!installedBinary()) {
  console.error("Electron binary is missing. Re-run pnpm install.");
  process.exit(1);
}
brandDevBundle();

process.exit(install.status ?? 0);

// Dev-only: give the local Electron.app the product's name so the macOS menu
// bar, Dock label and Cmd-Tab switcher say "Axiom" instead of "Electron"
// while developing. Packaged builds get this from electron-builder; this only
// touches the copy under node_modules. The plist is replaced (unlink + write)
// rather than edited in place so pnpm's hard-linked store copy stays pristine.
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, readFileSync, renameSync, unlinkSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));

export function productName() {
  const manifest = JSON.parse(readFileSync(resolve(here, "../package.json"), "utf8"));
  return manifest.productName ?? "Axiom";
}

export function electronBinary() {
  const electronDir = dirname(require.resolve("electron/package.json"));
  const pathFile = join(electronDir, "path.txt");
  if (!existsSync(pathFile)) return null;
  const relative = readFileSync(pathFile, "utf8").trim();
  const binary = join(electronDir, "dist", relative);
  return relative && existsSync(binary) ? binary : null;
}

export function brandDevBundle() {
  if (process.platform !== "darwin") return false;
  const binary = electronBinary();
  if (!binary) return false;
  // .../Electron.app/Contents/MacOS/Electron → .../Electron.app/Contents/Info.plist
  const plist = resolve(dirname(binary), "../Info.plist");
  if (!existsSync(plist)) return false;
  const name = productName();
  const current = execFileSync("/usr/libexec/PlistBuddy", ["-c", "Print :CFBundleName", plist], { encoding: "utf8" }).trim();
  if (current === name) return false;
  const staged = `${plist}.axiom-tmp`;
  copyFileSync(plist, staged);
  for (const key of ["CFBundleName", "CFBundleDisplayName"]) {
    execFileSync("/usr/libexec/PlistBuddy", ["-c", `Set :${key} ${name}`, staged]);
  }
  unlinkSync(plist);
  renameSync(staged, plist);
  console.log(`Named the dev Electron bundle "${name}" (${plist})`);
  return true;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  brandDevBundle();
}

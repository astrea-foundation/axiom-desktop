import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const APP_ID = "stream.axiom.desktop";
const DESKTOP_FILE = `${APP_ID}.desktop`;

if (process.platform !== "linux") {
  console.log("Axiom desktop metadata is only needed on Linux.");
  process.exit(0);
}

function execArgument(value) {
  return `"${value.replaceAll("%", "%%").replace(/[\\"`$]/g, "\\$&")}"`;
}

function desktopString(value) {
  return value.replaceAll("\\", "\\\\").replaceAll("\n", "\\n").replaceAll("\r", "\\r");
}

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopRoot = resolve(scriptDir, "..");
const workspaceRoot = resolve(desktopRoot, "../..");
const dataHome = process.env.XDG_DATA_HOME || join(homedir(), ".local", "share");
const applicationsDir = join(dataHome, "applications");
const iconsDir = join(dataHome, "icons", "hicolor", "512x512", "apps");
const desktopPath = join(applicationsDir, DESKTOP_FILE);
const iconPath = join(iconsDir, `${APP_ID}.png`);

const desktopEntry = `[Desktop Entry]
Type=Application
Name=Axiom
Comment=Secure AI coding desktop
Exec=pnpm --dir ${execArgument(workspaceRoot)} desktop
Path=${desktopString(workspaceRoot)}
Icon=${APP_ID}
Terminal=false
Categories=Development;
StartupNotify=true
StartupWMClass=Axiom
`;

await Promise.all([
  mkdir(applicationsDir, { recursive: true }),
  mkdir(iconsDir, { recursive: true }),
]);
await Promise.all([
  writeFile(desktopPath, desktopEntry, { encoding: "utf8", mode: 0o644 }),
  copyFile(join(desktopRoot, "resources", "icon.png"), iconPath),
]);

console.log(`Installed ${desktopPath}`);
console.log(`Installed ${iconPath}`);

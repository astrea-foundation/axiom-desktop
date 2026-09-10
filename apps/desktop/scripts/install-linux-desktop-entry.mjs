import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { APP_ID, APP_NAME, LINUX_DESKTOP_FILE } from "./desktop-identity.mjs";

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
const staging = process.argv.includes("--staging");
const desktopPath = join(applicationsDir, staging ? `${APP_ID}.staging.desktop` : LINUX_DESKTOP_FILE);
const iconPath = join(iconsDir, `${APP_ID}.png`);

const desktopEntry = `[Desktop Entry]
Type=Application
Name=${staging ? `${APP_NAME} Staging` : APP_NAME}
Comment=Private AI chat sealed on your machine
Exec=pnpm --dir ${execArgument(workspaceRoot)} ${staging ? "desktop:staging" : "desktop"}
Path=${desktopString(workspaceRoot)}
Icon=${APP_ID}
Terminal=false
Categories=Utility;
StartupNotify=true
StartupWMClass=${APP_NAME}
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

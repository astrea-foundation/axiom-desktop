import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { APP_ID, APP_NAME, LINUX_DESKTOP_FILE, setLinuxDesktopIdentity } from "../scripts/desktop-identity.mjs";

test("Linux launches replace the parent IDE identity without changing service/profile or activation settings", () => {
  const env: Record<string, string | undefined> = {
    CHROME_DESKTOP: "cafecode.desktop", BAMF_DESKTOP_FILE_HINT: "/applications/cafecode.desktop",
    GIO_LAUNCHED_DESKTOP_FILE: "/applications/cafecode.desktop", GIO_LAUNCHED_DESKTOP_FILE_PID: "123",
    AXIOM_DESKTOP_STAGING_PROFILE: "1", XDG_CONFIG_HOME: "/config", DESKTOP_STARTUP_ID: "activation",
  };
  setLinuxDesktopIdentity(env, "linux");
  assert.deepEqual(env, {
    CHROME_DESKTOP: LINUX_DESKTOP_FILE, AXIOM_DESKTOP_STAGING_PROFILE: "1",
    XDG_CONFIG_HOME: "/config", DESKTOP_STARTUP_ID: "activation",
  });
});

test("macOS and Windows launch environments remain unchanged", () => {
  for (const platform of ["darwin", "win32"]) {
    const env = { CHROME_DESKTOP: "parent.desktop", GIO_LAUNCHED_DESKTOP_FILE: "parent.desktop" };
    const before = { ...env };
    setLinuxDesktopIdentity(env, platform);
    assert.deepEqual(env, before);
  }
});

test("Linux packaged metadata matches the runtime and development launcher", () => {
  const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  assert.equal(manifest.productName, APP_NAME);
  assert.equal(manifest.desktopName, LINUX_DESKTOP_FILE);
  const packaging = readFileSync(new URL("../electron-builder.yml", import.meta.url), "utf8");
  assert.match(packaging, new RegExp(`^appId: ${APP_ID.replaceAll(".", "\\.")}$`, "m"));
  assert.match(packaging, /^  syncDesktopName: true$/m);
  assert.match(packaging, new RegExp(`^      StartupWMClass: ${APP_NAME}$`, "m"));
});

test("installer writes Axiom's launcher and its own icon; staging remains opt-in", { skip: process.platform !== "linux" }, (t) => {
  const dataDir = mkdtempSync(join(tmpdir(), "axiom-identity-test-"));
  t.after(() => rmSync(dataDir, { recursive: true, force: true }));
  const installer = fileURLToPath(new URL("../scripts/install-linux-desktop-entry.mjs", import.meta.url));
  for (const staging of [false, true]) {
    const result = spawnSync(process.execPath, [installer, ...(staging ? ["--staging"] : [])], {
      env: { ...process.env, XDG_DATA_HOME: dataDir }, encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    const filename = staging ? `${APP_ID}.staging.desktop` : LINUX_DESKTOP_FILE;
    const entry = readFileSync(join(dataDir, "applications", filename), "utf8");
    assert.match(entry, staging ? /^Name=Axiom Staging$/m : /^Name=Axiom$/m);
    assert.match(entry, /^StartupWMClass=Axiom$/m);
    assert.match(entry, new RegExp(`^Icon=${APP_ID.replaceAll(".", "\\.")}$`, "m"));
    assert.match(entry, staging ? / desktop:staging\n/ : / desktop\n/);
    assert.doesNotMatch(entry, /cafecode|cafe-code|Icon=electron/i);
    assert.deepEqual(
      readFileSync(join(dataDir, "icons", "hicolor", "512x512", "apps", `${APP_ID}.png`)),
      readFileSync(new URL("../resources/icon.png", import.meta.url)),
    );
  }
  const production = readFileSync(join(dataDir, "applications", LINUX_DESKTOP_FILE), "utf8");
  assert.match(production, /^Name=Axiom$/m);
  assert.match(production, / desktop\n/);
  assert.doesNotMatch(production, /desktop:staging/);
});

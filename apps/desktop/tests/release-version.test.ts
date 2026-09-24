import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";

const script = fileURLToPath(new URL("../scripts/set-release-version.mjs", import.meta.url));
const cliManifest = readFileSync(new URL("../../axiomcli/Cargo.toml", import.meta.url), "utf8");
const currentVersion = /^version\s*=\s*"([^"]+)"\s*$/m.exec(cliManifest)?.[1];
assert.match(currentVersion ?? "", /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);

function validate(version: string) {
  return spawnSync(process.execPath, [script, version, "--check-only"], {
    encoding: "utf8",
  });
}

test("release version accepts the current stable product version", () => {
  const result = validate(currentVersion!);
  assert.equal(result.status, 0, result.stderr);
});

test("release version rejects prerelease and build metadata", () => {
  for (const version of ["0.1.0-beta.1", "0.1.0+build.7", "0.1.0-beta.1+build.7"]) {
    const result = validate(version);
    assert.notEqual(result.status, 0, `${version} unexpectedly passed`);
    assert.match(result.stderr, /stable MAJOR\.MINOR\.PATCH/);
  }
});

test("release version rejects a stable tag that differs from AxiomCLI", () => {
  const differentStableVersion = currentVersion === "0.0.0" ? "0.0.1" : "0.0.0";
  const result = validate(differentStableVersion);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not match AxiomCLI product version/);
});

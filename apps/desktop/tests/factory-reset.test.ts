import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { FactoryResetController, factoryResetError, factoryResetRelaunch } from "../src/main/factory-reset";

test("reset hands off to an offline native helper, preserving each restart argument without a shell", () => {
  const result = factoryResetRelaunch({ sidecar: "/Axiom/bin/axiomcli", executable: "/Axiom/Axiom", profile: "local",
    userData: "/profile/Axiom Local", argv: ["electron", "/app with spaces", "--dev", "$(do-not-run)", "--axiom-factory-reset-error=old"] });
  assert.equal(result.execPath, "/Axiom/bin/axiomcli");
  assert.deepEqual(result.args, ["factory-reset", "--confirm", "--profile", "local", "--desktop-user-data", "/profile/Axiom Local",
    "--restart-executable", "/Axiom/Axiom", "--restart-arg=/app with spaces", "--restart-arg=--dev", "--restart-arg=$(do-not-run)"]);
});

test("reset requires exact confirmation and waits for all writers before scheduling restart", async () => {
  const calls: string[] = [];
  let finish!: () => void;
  const controller = new FactoryResetController({
    isInstalling: () => false,
    prepare: async () => { calls.push("prepare"); return () => { calls.push("relaunch"); }; },
    stop: () => new Promise<void>((resolve) => { calls.push("stop"); finish = resolve; }),
    quit: () => { calls.push("quit"); },
  });
  for (const invalid of [undefined, null, true, "reset", " RESET ", { confirmation: "RESET" }]) {
    await assert.rejects(controller.reset(invalid), /Type RESET/);
  }
  assert.deepEqual(calls, []);
  const pending = controller.reset("RESET");
  assert.equal(controller.isPending(), true);
  await Promise.resolve();
  assert.deepEqual(calls, ["prepare", "stop"]);
  await assert.rejects(controller.reset("RESET"), /already in progress/);
  finish();
  await pending;
  assert.deepEqual(calls, ["prepare", "stop", "relaunch", "quit"]);
});

test("installing updates prevent reset and preparation failures remain retryable without quitting", async () => {
  let installing = true, fail = true;
  const calls: string[] = [];
  const controller = new FactoryResetController({ isInstalling: () => installing,
    prepare: async () => { if (fail) throw new Error("native helper missing"); return () => { calls.push("relaunch"); }; },
    stop: async () => { calls.push("stop"); }, quit: () => { calls.push("quit"); } });
  await assert.rejects(controller.reset("RESET"), /update/);
  installing = false;
  await assert.rejects(controller.reset("RESET"), /helper missing/);
  assert.equal(controller.isPending(), false);
  assert.deepEqual(calls, []);
  fail = false;
  await controller.reset("RESET");
  assert.deepEqual(calls, ["stop", "relaunch", "quit"]);
});

test("native reset failures survive restart for a visible error instead of a false success", () => {
  const message = "Close other AxiomCLI processes, then retry.";
  assert.equal(factoryResetError(["app", `--axiom-factory-reset-error=${Buffer.from(message).toString("base64url")}`]), message);
  assert.equal(factoryResetError(["app"]), null);
});

test("named development profiles are forwarded as a separate validated native argument", () => {
  const result = factoryResetRelaunch({ sidecar: "/Axiom/axiomcli", executable: "/Axiom/Axiom",
    profile: "staging", profileName: "qa-reset", userData: "/profiles/Axiom Staging qa-reset", argv: ["app"] });
  assert.deepEqual(result.args.slice(0, 7), ["factory-reset", "--confirm", "--profile", "staging", "--profile-name", "qa-reset", "--desktop-user-data"]);
});

test("development reset reopens the Node launcher, including Vite, rather than Electron with a dead UI server", () => {
  const entry = fileURLToPath(new URL("../scripts/run-electron.mjs", import.meta.url));
  const result = factoryResetRelaunch({ sidecar: "/Axiom/axiomcli", executable: "/Axiom/Electron",
    profile: "system", userData: "/profile/Axiom", argv: ["electron", "/app", "--old-argument"],
    development: true, developmentRestart: JSON.stringify({ executable: process.execPath, args: [entry, "vite", "dev"] }),
  });
  assert.deepEqual(result.args.slice(result.args.indexOf("--restart-executable")), ["--restart-executable", process.execPath,
    `--restart-arg=${entry}`, "--restart-arg=vite", "--restart-arg=dev"]);
});

test("an unavailable development launcher refuses reset before stopping the app or deleting data", () => {
  const entry = fileURLToPath(new URL("../scripts/run-electron.mjs", import.meta.url));
  const options = { sidecar: "/Axiom/axiomcli", executable: "/Axiom/Electron", profile: "system" as const,
    userData: "/profile/Axiom", argv: ["electron", "/app"], development: true };
  for (const developmentRestart of [undefined, "bad json", "null", JSON.stringify({ executable: "relative", args: [entry, "vite", "dev"] }),
    JSON.stringify({ executable: process.execPath, args: [entry, "vite", "preview"] }),
    JSON.stringify({ executable: process.execPath, args: [entry + ".missing", "vite", "dev"] })]) {
    assert.throws(() => factoryResetRelaunch({ ...options, developmentRestart }), /Restart Axiom with pnpm desktop/);
  }
  // Installed/preview builds keep their native restart even if dev metadata is inherited.
  assert.doesNotThrow(() => factoryResetRelaunch({ ...options, development: false, developmentRestart: "bad json" }));
});

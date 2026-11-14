import assert from "node:assert/strict";
import test from "node:test";
import { electronLaunch } from "../scripts/launch-environment.mjs";

const executable = "/node with spaces/node";
const entry = "/checkout with spaces/scripts/run-electron.mjs";

for (const target of ["vite", "staging"]) test(`${target}: development reset starts the complete launcher with fresh renderer routing`, () => {
  const environment = { ELECTRON_RUN_AS_NODE: "1", ELECTRON_RENDERER_URL: "http://localhost:5173",
    AXIOM_DESKTOP_DEV_RESTART: "obsolete", AXIOM_DESKTOP_LOCAL_PROFILE_NAME: "qa-profile" };
  const launch = electronLaunch([target, "dev", "--host", "127.0.0.1"], environment, executable, entry);
  assert.deepEqual(launch.args, ["dev", "--host", "127.0.0.1"]);
  assert.equal(launch.env.ELECTRON_RUN_AS_NODE, undefined);
  assert.equal(launch.env.ELECTRON_RENDERER_URL, undefined);
  assert.equal(launch.env.AXIOM_DESKTOP_LOCAL_PROFILE_NAME, "qa-profile");
  assert.deepEqual(JSON.parse(launch.env.AXIOM_DESKTOP_DEV_RESTART), {
    executable, args: [entry, target, "dev", "--host", "127.0.0.1"],
  });
  assert.equal(environment.ELECTRON_RENDERER_URL, "http://localhost:5173", "do not mutate the parent environment");
});

test("native reset errors reach Electron without becoming Vite flags or recurring on the next reset", () => {
  const previous = "--axiom-factory-reset-error=previous";
  const failure = "--axiom-factory-reset-error=current";
  const launch = electronLaunch(["vite", "dev", failure], {
    ELECTRON_CLI_ARGS: JSON.stringify(["--inspect=127.0.0.1:9000", previous]),
  }, executable, entry);
  assert.deepEqual(launch.args, ["dev"]);
  assert.deepEqual(JSON.parse(launch.env.ELECTRON_CLI_ARGS), ["--inspect=127.0.0.1:9000", failure]);
  assert.deepEqual(JSON.parse(launch.env.AXIOM_DESKTOP_DEV_RESTART).args, [entry, "vite", "dev"]);
  const next = electronLaunch(["vite", "dev"], launch.env, executable, entry);
  assert.deepEqual(JSON.parse(next.env.ELECTRON_CLI_ARGS), ["--inspect=127.0.0.1:9000"]);
});

test("preview removes development-only restart and renderer state", () => {
  const launch = electronLaunch(["vite", "preview"], {
    AXIOM_DESKTOP_DEV_RESTART: "obsolete", ELECTRON_RENDERER_URL: "http://localhost:5173",
  }, executable, entry);
  assert.equal(launch.env.AXIOM_DESKTOP_DEV_RESTART, undefined);
  assert.equal(launch.env.ELECTRON_RENDERER_URL, undefined);
});

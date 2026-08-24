import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { AxiomAcpClient, AxiomStateStore } from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const binary = resolve(here, "../../../target/debug/axiomcli");

async function fixture(runner = "echo"): Promise<{
  client: AxiomAcpClient;
  workspace: string;
}> {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-acp-client-"));
  const workspace = resolve(root, "workspace");
  await mkdir(workspace);
  await writeFile(resolve(workspace, "README.md"), "fixture\n");
  const client = new AxiomAcpClient({
    command: binary,
    args: ["acp"],
    env: {
      AXIOMCLI_TEST_RUNNER: runner,
      XDG_CONFIG_HOME: resolve(root, "config"),
      XDG_DATA_HOME: resolve(root, "data"),
    },
  });
  await client.initialize();
  return { client, workspace };
}

test("production TypeScript client negotiates, streams and controls typed settings", async () => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    const models = await client.listModels();
    assert.ok(models.models.some((model) => model.id === "alternate-model"));
    await client.setSettings({
      sessionId: created.sessionId,
      model: "alternate-model",
      thinkingLevel: "high",
      permissionProfile: "observe",
    });
    const result = await client.prompt(created.sessionId, "hello from desktop");
    assert.equal(result.stopReason, "end_turn");
    const state = client.getState().sessions[created.sessionId];
    assert.equal(state?.settings?.permissionProfile, "observe");
    assert.ok(state?.timeline.some((item) => item.text.includes("hello from desktop")));
  } finally {
    await client.close();
  }
});

test("permission requests remain owned by AxiomCLI and resolve through the client", async () => {
  const { client, workspace } = await fixture("approval");
  try {
    const created = await client.newSession(workspace);
    client.once("interaction", (interaction) => {
      assert.equal(interaction.kind, "permission");
      client.resolvePermission(interaction.id, {
        outcome: "selected",
        optionId: "allow_once",
      });
    });
    const result = await client.prompt(created.sessionId, "approve fixture");
    assert.equal(result.stopReason, "end_turn");
    assert.equal(client.getState().sessions[created.sessionId]?.interactions.length, 0);
  } finally {
    await client.close();
  }
});

test("cancelling a session fail-closes and clears its pending permission", async () => {
  const { client, workspace } = await fixture("approval");
  try {
    const created = await client.newSession(workspace);
    const interaction = once(client, "interaction");
    const prompt = client.prompt(created.sessionId, "cancel approval fixture");
    const [pending] = await interaction;
    assert.equal(pending.kind, "permission");
    await client.cancel(created.sessionId);
    const result = await prompt;
    // A cancellation racing the explicit denial may be reported by ACP as
    // cancelled or as a normally completed denied turn. Neither path grants
    // authority or leaves an interaction behind.
    assert.ok(["cancelled", "end_turn"].includes(result.stopReason));
    assert.equal(client.getState().sessions[created.sessionId]?.interactions.length, 0);
  } finally {
    await client.close();
  }
});

test("extension sequence gaps are detected and snapshots can repair state", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sessionId: "session",
    sequence: 1,
    occurredAt: "2026-08-24T00:00:00Z",
    event: { kind: "progress", message: "one" },
  });
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sessionId: "session",
    sequence: 3,
    occurredAt: "2026-08-24T00:00:01Z",
    event: { kind: "progress", message: "three" },
  });
  assert.equal(store.snapshot().sessions.session?.needsResync, true);
  store.setSnapshot({
    runtimeInstanceId: "runtime",
    sequence: 3,
    sessionId: "session",
    cwd: "/tmp",
    lifecycle: "ready",
    settings: { model: "model", thinkingLevel: "medium", permissionProfile: "confirm" },
    security: { state: "verified" },
    hasPrompt: true,
  });
  assert.equal(store.snapshot().sessions.session?.needsResync, false);
});

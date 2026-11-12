import assert from "node:assert/strict";
import test from "node:test";
import { AxiomStateStore, type AccountStatus } from "../src/index.js";

function progress(revision: number, phase: NonNullable<AccountStatus["storageMigration"]>["phase"]): AccountStatus {
  return { revision, state: phase === "failed" ? "unavailable" : "starting", storageMigration: { phase } };
}

test("migration stays outside an account context and late progress cannot replace a completed account", () => {
  const store = new AxiomStateStore();
  store.setAccount({ revision: 1, state: "valid", account: { id: "old-account", linkedMethods: [] } });
  store.addSession("old-thread", "");
  store.setAccount(progress(2, "encrypting"));
  const transition = store.accountContextToken();
  assert.deepEqual(store.snapshot().sessions, {});
  assert.equal(store.snapshot().account?.account, undefined);
  store.setAccount({ ...progress(3, "moving_media"), storageMigration: { phase: "moving_media", completed: 10, total: 20 } });
  assert.equal(store.accountContextToken(), transition, "phase updates do not create new account generations");
  store.setAccount(progress(4, "finalizing"));
  store.setAccount({ revision: 5, state: "valid", account: { id: "new-account", linkedMethods: [] } });
  store.addSession("new-thread", "");
  store.setAccount(progress(4, "checking"));
  assert.equal(store.snapshot().account?.state, "valid");
  assert.equal(store.snapshot().account?.storageMigration, undefined);
  assert.ok(store.snapshot().sessions["new-thread"]);
});

test("migration failure can be retried but stale failure cannot hide later readiness", () => {
  const store = new AxiomStateStore();
  store.setAccount(progress(1, "encrypting"));
  store.setAccount({ ...progress(2, "failed"), detail: "Synthetic failure; data retained" });
  assert.equal(store.snapshot().account?.storageMigration?.phase, "failed");
  store.setAccount(progress(3, "checking"));
  store.setAccount({ revision: 4, state: "valid", account: { id: "restored-account", linkedMethods: [] } });
  store.setAccount(progress(2, "failed"));
  assert.equal(store.snapshot().account?.account?.id, "restored-account");
});

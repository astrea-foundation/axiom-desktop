import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { AxiomAcpClient } from "../src/client.js";
import type { BillingStatusResponse } from "../src/generated/protocol.js";

function fixture(t: TestContext) {
  // No native credentials or HTTP services are involved. Hold actual ACP
  // request promises so overlapping callers and account changes are explicit.
  const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
  t.after(() => client.close());
  client.state.setAccount({ revision: 1, state: "valid", account: { id: "account-a", linkedMethods: [] } });
  const pending: { resolve: (result: BillingStatusResponse) => void; reject: (error: Error) => void }[] = [];
  t.mock.method(client.process, "request", async (method: string) => {
    assert.equal(method, "_axiom/billing/status");
    return new Promise<BillingStatusResponse>((resolve, reject) => pending.push({ resolve, reject }));
  });
  const response = (revision = 1): BillingStatusResponse => ({ status: {
    revision, ledgerSequence: revision, currency: "microUSD", postedMicrousd: 2_000_000,
    availableMicrousd: 2_000_000, trialMicrousd: 1_000_000, paidMicrousd: 1_000_000,
  } });
  return { client, pending, response };
}

test("overlapping startup, menu and balance refreshes share one native request", async (t) => {
  const f = fixture(t);
  const requests = [f.client.billingStatus(), f.client.billingStatus(), f.client.billingStatus()];
  assert.equal(f.pending.length, 1, "presentation surfaces must not multiply the same account request");
  f.pending[0]!.resolve(f.response());
  await Promise.all(requests);
  assert.equal(f.client.getState().billing?.availableMicrousd, 2_000_000);
  const next = f.client.billingStatus();
  assert.equal(f.pending.length, 2, "settled results are not a stale balance cache");
  f.pending[1]!.resolve(f.response(2));
  await next;
});

test("a shared failed refresh rejects all callers and permits a later retry", async (t) => {
  const f = fixture(t);
  const requests = [f.client.billingStatus(), f.client.billingStatus()];
  const outcomes = Promise.allSettled(requests);
  assert.equal(f.pending.length, 1);
  f.pending[0]!.reject(new Error("temporary billing failure"));
  assert.ok((await outcomes).every(result => result.status === "rejected"));
  assert.equal(f.client.getState().billing, null);
  const retry = f.client.billingStatus();
  assert.equal(f.pending.length, 2);
  f.pending[1]!.resolve(f.response());
  await retry;
});

test("a new account never joins or publishes an old account's pending balance", async (t) => {
  const f = fixture(t);
  const old = f.client.billingStatus();
  const oldOutcome = assert.rejects(old, /account|context/i);
  f.client.state.setAccount({ revision: 2, state: "valid", account: { id: "account-b", linkedMethods: [] } });
  const current = f.client.billingStatus();
  assert.equal(f.pending.length, 2);
  f.pending[0]!.resolve(f.response(99));
  await oldOutcome;
  assert.equal(f.client.getState().billing, null);
  const joined = f.client.billingStatus();
  assert.equal(f.pending.length, 2, "old completion cannot clear the new account's shared request");
  f.pending[1]!.resolve(f.response());
  await Promise.all([current, joined]);
  assert.equal(f.client.getState().billing?.revision, 1);
});

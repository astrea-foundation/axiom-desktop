import assert from "node:assert/strict";
import test from "node:test";

import {
  BillingInvoiceRequestTracker,
  type InvoiceRequestStorage,
} from "../src/renderer/src/billingInvoiceRequest.js";

class MemoryStorage implements InvoiceRequestStorage {
  readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

function ids(...values: string[]): () => string {
  return () => {
    const value = values.shift();
    assert.ok(value, "unexpected id generation");
    return value;
  };
}

test("lost invoice responses reuse one durable account-and-amount idempotency key", async () => {
  const storage = new MemoryStorage();
  const first = new BillingInvoiceRequestTracker(ids("request-1"), storage);
  const sent: string[] = [];

  await assert.rejects(first.submit("account-a", 2_500, async (requestId) => {
    sent.push(requestId);
    throw new Error("response was lost after dispatch");
  }));
  assert.equal(first.snapshot()?.clientRequestId, "request-1");

  // A renderer reload reconstructs the tracker from bounded non-secret local
  // metadata and must still retry the server's potentially committed intent.
  const reloaded = new BillingInvoiceRequestTracker(ids("request-2"), storage);
  await reloaded.submit("account-a", 2_500, async (requestId) => {
    sent.push(requestId);
  });
  assert.deepEqual(sent, ["request-1", "request-1"]);
  assert.equal(reloaded.snapshot(), null, "a definitive response completes the intent");

  await reloaded.submit("account-a", 2_500, async (requestId) => {
    sent.push(requestId);
  });
  assert.equal(sent.at(-1), "request-2", "the next explicit invoice gets a fresh key");
});

test("amount and account changes explicitly abandon only the prior pending intent", async () => {
  const tracker = new BillingInvoiceRequestTracker(
    ids("request-a", "request-b", "request-c"),
    new MemoryStorage(),
  );
  const fail = async () => { throw new Error("timeout"); };

  await assert.rejects(tracker.submit("account-a", 2_500, fail));
  assert.equal(tracker.snapshot()?.clientRequestId, "request-a");
  await assert.rejects(tracker.submit("account-a", 3_000, fail));
  assert.equal(tracker.snapshot()?.clientRequestId, "request-b");
  await assert.rejects(tracker.submit("account-a", 2_500, fail));
  assert.equal(tracker.snapshot()?.clientRequestId, "request-c");

  tracker.reconcileAccount(undefined);
  assert.equal(tracker.snapshot()?.clientRequestId, "request-c", "transient startup retains retry state");
  tracker.reconcileAccount("account-b");
  assert.equal(tracker.snapshot(), null);
});

test("a stale completion from account A cannot clear account B retry state", async () => {
  const tracker = new BillingInvoiceRequestTracker(
    ids("request-a", "request-b"),
    new MemoryStorage(),
  );
  let releaseA!: () => void;
  const delayedA = new Promise<void>((resolve) => { releaseA = resolve; });
  const first = tracker.submit("account-a", 2_500, async () => delayedA);

  tracker.reconcileAccount("account-b");
  await assert.rejects(tracker.submit("account-b", 2_500, async () => {
    throw new DOMException("cancelled", "AbortError");
  }));
  assert.equal(tracker.snapshot()?.clientRequestId, "request-b");

  releaseA();
  await first;
  assert.equal(
    tracker.snapshot()?.clientRequestId,
    "request-b",
    "the exact-request completion check preserves the replacement account",
  );

  let retryId = "";
  await tracker.submit("account-b", 2_500, async (requestId) => { retryId = requestId; });
  assert.equal(retryId, "request-b");
});

test("a durable-storage failure prevents invoice dispatch", async () => {
  const storage = new MemoryStorage();
  storage.setItem = () => { throw new Error("quota denied"); };
  const tracker = new BillingInvoiceRequestTracker(ids("request-a"), storage);
  let dispatched = false;

  await assert.rejects(
    tracker.submit("account-a", 2_500, async () => { dispatched = true; }),
    /retry storage is unavailable.*no invoice was sent/i,
  );
  assert.equal(dispatched, false);
  assert.equal(tracker.snapshot(), null);
});

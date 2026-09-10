import assert from "node:assert/strict";
import test from "node:test";
import type { BillingInvoice } from "@axiom/axiom-acp-client";
import {
  BillingCheckoutFlow,
  monitorBillingInvoice,
  type BillingCheckoutApi,
} from "../src/renderer/src/billingCheckout.js";
import { BillingInvoiceRequestTracker } from "../src/renderer/src/billingInvoiceRequest.js";
import { activeInvoicePaymentUrl, requirePaymentAccount } from "../src/main/service-origins.js";

function pendingInvoice(): BillingInvoice {
  return {
    revision: 1, id: "a".repeat(32), amountUsdCents: 2_500, state: "pending",
    expiresAt: new Date(Date.now() + 1_800_000).toISOString(),
    payUrl: "https://shieldz.cash/pay/test_invoice",
  };
}

function fixture() {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    removeItem: (key: string) => { values.delete(key); },
  };
  let sequence = 0;
  const createId = () => `request-${++sequence}`;
  const tracker = new BillingInvoiceRequestTracker(createId, storage);
  const flow = new BillingCheckoutFlow(tracker);
  const state: Awaited<ReturnType<BillingCheckoutApi["getState"]>> = {
    connected: true,
    account: { revision: 1, state: "valid", account: { id: "account-a", linkedMethods: [] } },
    invoice: null,
  };
  const events: string[] = [];
  const invoice = pendingInvoice();
  const api: BillingCheckoutApi = {
    getState: async () => state,
    createBillingInvoice: async (_amount, requestId, accountId) => {
      requirePaymentAccount(accountId, state);
      events.push(`create:${requestId}`);
      state.invoice = invoice;
      return { invoice };
    },
    billingInvoiceStatus: async () => {
      events.push("status");
      return { invoice };
    },
    openBillingPayment: async (url, accountId) => {
      activeInvoicePaymentUrl(url, state, accountId);
      assert.equal(url, invoice.payUrl);
      events.push("open");
    },
  };
  return { flow, tracker, state, api, invoice, events, values, storage, createId };
}

test("Top up creates and immediately opens checkout; repeated clicks are single-flight", async () => {
  const { flow, tracker, api, events } = fixture();
  const first = flow.topUp("account-a", 2_500, api);
  assert.equal(flow.topUp("account-a", 2_500, api), first);
  await first;
  assert.deepEqual(events, ["create:request-1", "open"]);
  assert.equal(tracker.snapshot(), null);
});

test("reopening a pending checkout refreshes it without creating another invoice", async () => {
  const { flow, api, events, state, invoice } = fixture();
  state.invoice = invoice;
  await flow.topUp("account-a", 2_500, api);
  assert.deepEqual(events, ["status", "open"]);
});

test("a failed browser launch keeps the invoice retry key across renderer reload", async () => {
  const { flow, tracker, api, events, state, storage, createId, values } = fixture();
  const open = api.openBillingPayment;
  api.openBillingPayment = async () => { throw new Error("OS browser launcher failed"); };
  await assert.rejects(flow.topUp("account-a", 2_500, api), /Reopen checkout/);
  assert.equal(tracker.snapshot()?.clientRequestId, "request-1");
  assert.ok([...values.values()].every((value) => !value.includes("https://")), "checkout URLs are not persisted in renderer retry storage");

  state.invoice = null;
  api.openBillingPayment = open;
  const reloaded = new BillingCheckoutFlow(new BillingInvoiceRequestTracker(createId, storage));
  await reloaded.topUp("account-a", 2_500, api);
  assert.deepEqual(events, ["create:request-1", "create:request-1", "open"]);
});

test("lost creation response retries the same invoice before opening anything", async () => {
  const { flow, api, events } = fixture();
  const create = api.createBillingInvoice;
  let fail = true;
  api.createBillingInvoice = async (...args) => {
    const result = await create(...args);
    if (fail) { fail = false; throw new Error("lost response"); }
    return result;
  };
  await assert.rejects(flow.topUp("account-a", 2_500, api), /lost response/);
  await flow.topUp("account-a", 2_500, api);
  assert.deepEqual(events, ["create:request-1", "create:request-1", "open"]);
});

test("a pending invoice paid in the browser is not reopened or replaced", async () => {
  const { flow, api, state, invoice, events } = fixture();
  state.invoice = invoice;
  api.billingInvoiceStatus = async () => {
    events.push("status");
    return { invoice: { ...invoice, state: "paid", payUrl: null } };
  };
  await flow.topUp("account-a", 2_500, api);
  assert.deepEqual(events, ["status"]);
});

test("terminal creation responses clear retry state and never open checkout", async () => {
  for (const status of ["paid", "expired", "failed", "review"] as const) {
    const { flow, tracker, api, invoice, events } = fixture();
    invoice.state = status;
    invoice.payUrl = null;
    if (status === "paid") await flow.topUp("account-a", 2_500, api);
    else await assert.rejects(flow.topUp("account-a", 2_500, api), /ended|under review/);
    assert.deepEqual(events, ["create:request-1"]);
    assert.equal(tracker.snapshot(), null);
  }
});

test("preparing checkout without a URL retains the retry key", async () => {
  const { flow, tracker, api, invoice, events } = fixture();
  invoice.state = "creating";
  invoice.payUrl = null;
  await assert.rejects(flow.topUp("account-a", 2_500, api), /still being prepared/);
  assert.equal(tracker.snapshot()?.clientRequestId, "request-1");
  assert.deepEqual(events, ["create:request-1"]);
});

test("top-up rejects stale account context and a concurrent different amount", async () => {
  const { flow, api, state, events } = fixture();
  await assert.rejects(flow.topUp("account-b", 2_500, api), /Sign in/);
  assert.deepEqual(events, []);
  const first = flow.topUp("account-a", 2_500, api);
  await assert.rejects(flow.topUp("account-a", 3_000, api), /Please wait/);
  await first;
  state.connected = false;
  await assert.rejects(flow.topUp("account-a", 2_500, api), /Sign in/);
});

test("account changes after a renderer snapshot cannot dispatch an invoice for the new account", async () => {
  const { flow, api, state, events } = fixture();
  api.getState = async () => {
    const snapshot = structuredClone(state);
    state.account = { revision: 2, state: "valid", account: { id: "account-b", linkedMethods: [] } };
    return snapshot;
  };
  await assert.rejects(flow.topUp("account-a", 2_500, api), /account changed/);
  assert.deepEqual(events, []);
});

test("account changes after creation cannot open a late checkout", async () => {
  const { flow, api, state, events } = fixture();
  const create = api.createBillingInvoice;
  api.createBillingInvoice = async (...args) => {
    const result = await create(...args);
    state.account = { revision: 2, state: "valid", account: { id: "account-b", linkedMethods: [] } };
    state.invoice = null;
    return result;
  };
  await assert.rejects(flow.topUp("account-a", 2_500, api), /Could not open checkout/);
  assert.deepEqual(events, ["create:request-1"]);
});

function scheduler() {
  const queued = new Map<number, { callback: () => void; delay: number }>();
  let id = 0;
  return {
    queued,
    schedule: (callback: () => void, delay: number) => {
      const key = ++id;
      queued.set(key, { callback, delay });
      return () => { queued.delete(key); };
    },
  };
}

test("background invoice confirmation refreshes balance once and stops", async () => {
  const timers = scheduler();
  const events: string[] = [];
  const invoice = pendingInvoice();
  const monitor = monitorBillingInvoice(invoice, {
    billingInvoiceStatus: async () => {
      events.push("status");
      return { invoice: { ...invoice, state: "paid", payUrl: null } };
    },
    billingStatus: async () => { events.push("balance"); },
  }, timers.schedule);
  assert.equal([...timers.queued.values()][0]?.delay, 5_000);
  await monitor.refresh(); // Same refresh used on returning to the desktop window.
  await monitor.refresh();
  assert.deepEqual(events, ["status", "balance"]);
  assert.equal(timers.queued.size, 0);
});

test("confirmed payments retry a failed balance refresh without polling or reopening checkout", async () => {
  const timers = scheduler();
  let attempts = 0;
  const monitor = monitorBillingInvoice({ ...pendingInvoice(), state: "paid" }, {
    billingInvoiceStatus: async () => { throw new Error("paid invoices need no further poll"); },
    billingStatus: async () => { if (++attempts === 1) throw new Error("offline"); },
  }, timers.schedule);
  assert.equal([...timers.queued.values()][0]?.delay, 0);
  await monitor.refresh();
  assert.equal([...timers.queued.values()][0]?.delay, 5_000);
  await monitor.refresh();
  assert.equal(attempts, 2);
  assert.equal(timers.queued.size, 0);
});

test("pending status errors retry without inventing credit", async () => {
  const timers = scheduler();
  let attempts = 0;
  const monitor = monitorBillingInvoice(pendingInvoice(), {
    billingInvoiceStatus: async () => { attempts++; throw new Error("offline"); },
    billingStatus: async () => { assert.fail("no confirmed payment"); },
  }, timers.schedule);
  await monitor.refresh();
  assert.equal(attempts, 1);
  assert.equal(timers.queued.size, 1);
  monitor.stop();
  assert.equal(timers.queued.size, 0);
});

test("polling is single-flight and account-switch cleanup prevents late balance work", async () => {
  const timers = scheduler();
  const invoice = pendingInvoice();
  let finish!: () => void;
  const delayed = new Promise<void>((resolve) => { finish = resolve; });
  let calls = 0;
  const monitor = monitorBillingInvoice(invoice, {
    billingInvoiceStatus: async () => {
      calls++;
      await delayed;
      return { invoice: { ...invoice, state: "paid" } };
    },
    billingStatus: async () => { assert.fail("account context was replaced"); },
  }, timers.schedule);
  const pending = monitor.refresh();
  const duplicate = monitor.refresh();
  assert.equal(duplicate, pending);
  assert.equal(calls, 1);
  const drained = monitor.stop();
  let drainedBeforeResponse = false;
  void drained.then(() => { drainedBeforeResponse = true; });
  await Promise.resolve();
  assert.equal(drainedBeforeResponse, false, "checkout must wait for an outstanding status RPC");
  finish();
  await Promise.all([pending, duplicate, drained]);
  assert.equal(timers.queued.size, 0);
});

test("expired and failed invoices stop polling without refreshing credit", async () => {
  for (const status of ["expired", "failed", "review"] as const) {
    const timers = scheduler();
    const invoice = pendingInvoice();
    const monitor = monitorBillingInvoice(invoice, {
      billingInvoiceStatus: async () => ({ invoice: { ...invoice, state: status } }),
      billingStatus: async () => { assert.fail("payment is not confirmed"); },
    }, timers.schedule);
    await monitor.refresh();
    assert.equal(timers.queued.size, 0);
  }
});

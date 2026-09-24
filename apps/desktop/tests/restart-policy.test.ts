import assert from "node:assert/strict";
import test from "node:test";
import { RestartPolicy, type RestartScheduler } from "../src/main/restart-policy";

class FakeScheduler implements RestartScheduler {
  private nextHandle = 1;
  readonly pending = new Map<number, { callback: () => void; delayMs: number }>();
  readonly delays: number[] = [];

  schedule(callback: () => void, delayMs: number): number {
    const handle = this.nextHandle++;
    this.delays.push(delayMs);
    this.pending.set(handle, { callback, delayMs });
    return handle;
  }

  cancel(handle: unknown): void {
    this.pending.delete(handle as number);
  }

  runNext(): void {
    const next = [...this.pending.entries()].sort(([left], [right]) => left - right)[0];
    assert.ok(next, "expected a pending timer");
    this.pending.delete(next[0]);
    next[1].callback();
  }
}

test("immediate post-ready crashes exhaust the bounded restart budget", () => {
  const scheduler = new FakeScheduler();
  const policy = new RestartPolicy({ scheduler, stabilityWindowMs: 30_000 });
  let restarts = 0;

  for (let failure = 0; failure < 3; failure += 1) {
    policy.markReady(() => true);
    assert.equal(policy.scheduleRestart(() => { restarts += 1; }), true);
    scheduler.runNext();
  }

  policy.markReady(() => true);
  assert.equal(policy.scheduleRestart(() => { restarts += 1; }), false);
  assert.equal(restarts, 3);
  assert.equal(policy.failures(), 3);
  assert.deepEqual(scheduler.delays.filter((delay) => delay < 30_000), [250, 500, 1_000]);
});

test("a stable ready window restores the restart budget", () => {
  const scheduler = new FakeScheduler();
  const policy = new RestartPolicy({ scheduler, stabilityWindowMs: 30_000 });

  assert.equal(policy.scheduleRestart(() => undefined), true);
  scheduler.runNext();
  assert.equal(policy.failures(), 1);

  policy.markReady(() => true);
  scheduler.runNext();
  assert.equal(policy.failures(), 0);
  assert.equal(policy.scheduleRestart(() => undefined), true);
  assert.equal(scheduler.delays.at(-1), 250);
});

test("shutdown cancellation removes restart and stability timers", () => {
  const scheduler = new FakeScheduler();
  const policy = new RestartPolicy({ scheduler });
  policy.markReady(() => true);
  assert.equal(policy.scheduleRestart(() => undefined), true);
  assert.equal(scheduler.pending.size, 1);
  policy.cancelAll();
  assert.equal(scheduler.pending.size, 0);
});

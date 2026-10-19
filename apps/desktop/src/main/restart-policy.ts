export interface RestartScheduler {
  schedule(callback: () => void, delayMs: number): unknown;
  cancel(handle: unknown): void;
}

const systemScheduler: RestartScheduler = {
  schedule: (callback, delayMs) => setTimeout(callback, delayMs),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface RestartPolicyOptions {
  maxFailures?: number;
  baseDelayMs?: number;
  stabilityWindowMs?: number;
  scheduler?: RestartScheduler;
}

export class RestartPolicy {
  private readonly maxFailures: number;
  private readonly baseDelayMs: number;
  private readonly stabilityWindowMs: number;
  private readonly scheduler: RestartScheduler;
  private failureCount = 0;
  private restartTimer: unknown | null = null;
  private stabilityTimer: unknown | null = null;

  constructor(options: RestartPolicyOptions = {}) {
    this.maxFailures = options.maxFailures ?? 3;
    this.baseDelayMs = options.baseDelayMs ?? 250;
    this.stabilityWindowMs = options.stabilityWindowMs ?? 30_000;
    this.scheduler = options.scheduler ?? systemScheduler;
  }

  scheduleRestart(restart: () => void): boolean {
    this.cancelStabilityWindow();
    if (this.restartTimer !== null || this.failureCount >= this.maxFailures) return false;
    const delay = this.baseDelayMs * 2 ** this.failureCount;
    this.failureCount += 1;
    this.restartTimer = this.scheduler.schedule(() => {
      this.restartTimer = null;
      restart();
    }, delay);
    return true;
  }

  markReady(isStillReady: () => boolean): void {
    this.cancelStabilityWindow();
    this.stabilityTimer = this.scheduler.schedule(() => {
      this.stabilityTimer = null;
      if (isStillReady()) this.failureCount = 0;
    }, this.stabilityWindowMs);
  }

  cancelStabilityWindow(): void {
    if (this.stabilityTimer !== null) this.scheduler.cancel(this.stabilityTimer);
    this.stabilityTimer = null;
  }

  cancelAll(): void {
    if (this.restartTimer !== null) this.scheduler.cancel(this.restartTimer);
    this.restartTimer = null;
    this.cancelStabilityWindow();
  }

  failures(): number {
    return this.failureCount;
  }
}

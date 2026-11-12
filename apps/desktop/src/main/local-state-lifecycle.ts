/** Serializes ordinary close/quit and account transitions around renderer ACKs.
 * Repeated close requests share one barrier; quit supersedes window close. */
export class LocalStateLifecycle<Window> {
  private pending: Promise<unknown> | null = null;
  private windows = new Set<Window>();
  private quitting = false;
  private cancelAccount?: () => Promise<unknown>;
  constructor(private host: {
    flush(reason: "close" | "account"): Promise<void>;
    release(): void;
    close(window: Window): void;
    stop(): Promise<void>;
    quit(): void;
    failed(error: unknown): void;
  }) {}
  get busy(): boolean { return this.pending !== null; }
  close(window: Window): Promise<unknown> { this.windows.add(window); this.cancelTransition(); return this.drain(); }
  quit(): Promise<unknown> { this.quitting = true; this.cancelTransition(); return this.drain(); }
  private cancelTransition(): void {
    const cancel = this.cancelAccount; this.cancelAccount = undefined;
    if (cancel) void cancel().catch(() => undefined);
  }
  private drain(): Promise<unknown> {
    if (this.pending) return this.pending;
    const work = Promise.resolve().then(async () => {
      await this.host.flush("close");
      if (!this.quitting) {
        const windows = [...this.windows]; this.windows.clear();
        for (const window of windows) this.host.close(window);
      }
      // Closing the last window can synchronously request app.quit().
      if (this.quitting) {
        this.windows.clear();
        await this.host.stop();
        this.quitting = false;
        this.host.quit();
      }
    }).catch(error => {
      this.windows.clear(); this.quitting = false;
      this.host.failed(error);
    }).finally(() => {
      this.host.release(); this.pending = null;
      if (this.quitting || this.windows.size) void this.drain();
    });
    this.pending = work;
    return work;
  }
  async account<T>(operation: () => Promise<T>, cancel?: () => Promise<unknown>): Promise<T> {
    if (this.pending) throw new Error("Axiom is saving local data. Wait for it to finish, then retry.");
    const work = Promise.resolve().then(async () => {
      await this.host.flush("account");
      // If closing won while the flush was in progress, never start a login.
      if (this.quitting || this.windows.size) throw new Error("Account change cancelled while closing Axiom.");
      this.cancelAccount = cancel;
      return operation();
    });
    this.pending = work;
    try { return await work; }
    finally {
      this.cancelAccount = undefined;
      this.host.release(); this.pending = null;
      if (this.quitting || this.windows.size) void this.drain();
    }
  }
}

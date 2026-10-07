type LoadAttempt = { state: "loading" | "loaded" | "failed" };

export type ThreadLoadResult =
  | { state: "ignored" }
  | { state: "loaded" }
  | { state: "failed"; error: unknown };

/** Single-flight loads with explicit retry and deletion invalidation. */
export class ThreadLoadTracker {
  private attempts = new Map<string, LoadAttempt>();
  private blocked = new Set<string>();

  isBlocked(id: string): boolean {
    return this.blocked.has(id);
  }

  block(id: string): void {
    this.blocked.add(id);
    this.attempts.delete(id);
  }

  unblock(id: string): void {
    this.blocked.delete(id);
    this.attempts.delete(id);
  }

  reset(): void {
    this.attempts.clear();
    this.blocked.clear();
  }

  async load(id: string, request: () => Promise<unknown>, retry = false): Promise<ThreadLoadResult> {
    const previous = this.attempts.get(id);
    if (this.blocked.has(id) || (previous && (previous.state === "loading" || !retry))) {
      return { state: "ignored" };
    }
    const attempt: LoadAttempt = { state: "loading" };
    this.attempts.set(id, attempt);
    try {
      await request();
      if (this.attempts.get(id) !== attempt) return { state: "ignored" };
      attempt.state = "loaded";
      return { state: "loaded" };
    } catch (error) {
      if (this.attempts.get(id) !== attempt) return { state: "ignored" };
      attempt.state = "failed";
      return { state: "failed", error };
    }
  }
}

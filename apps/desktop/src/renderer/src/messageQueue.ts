import type { ClientState, PromptResult, SteerTurnRequest, SteerTurnResponse } from "@axiom/axiom-acp-client";
import type { ConsentStorage } from "./webConsent";

export interface QueuedMessage {
  id: string;
  threadId: string;
  text: string;
  webEnabled: boolean;
  agentRevision: number;
  status: "queued" | "sending" | "steering";
  error?: string;
}
export interface QueueApi {
  getState(): Promise<ClientState>;
  promptWithWebConsent(thread: string, text: string, id: string, web: boolean, agentRevision: number): Promise<PromptResult>;
  steer(request: SteerTurnRequest): Promise<SteerTurnResponse>;
}
const MAX_MESSAGES = 32;
const MAX_BYTES = 64 * 1024;
const key = (account: string) => `axiom.message-queue.v1:${encodeURIComponent(account)}`;

/** On-device outbox. A native transcript acknowledgement, not a UI timeout,
 * removes a message. No background replay after account/runtime changes. */
export class MessageQueue {
  private account: string | null = null;
  private runtime: string | null = null;
  private generation = 0;
  private state: ClientState | null = null;
  private api: QueueApi | null = null;
  private items: QueuedMessage[] = [];
  private paused = new Set<string>();
  private operations = new Map<string, { threadId: string; steering: boolean }>();

  constructor(private storage: ConsentStorage | null, private changed: () => void, private reportError: (message: string) => void) {}

  list(threadId: string | null, accountId = this.account): QueuedMessage[] {
    if (accountId !== this.account) return [];
    return this.items.filter((item) => item.threadId === threadId).map((item) => ({ ...item }));
  }
  isPaused(threadId: string): boolean { return this.paused.has(threadId); }
  dispose(): void {
    this.generation++; this.account = null; this.runtime = null; this.api = null; this.state = null;
    this.operations.clear();
  }
  private persist(): void {
    if (!this.storage || !this.account) throw new Error("The message queue could not be saved. No new message was sent.");
    this.storage.setItem(key(this.account), JSON.stringify(this.items));
  }
  private save(): boolean {
    try { this.persist(); return true; }
    catch { this.reportError("The message queue could not be saved. Sending is paused; please keep Axiom open and try again."); return false; }
  }

  sync(state: ClientState, api: QueueApi | null): void {
    const account = state.connected && state.account?.state === "valid" ? state.account.account?.id ?? null : null;
    const runtime = account ? state.runtimeInstanceId : null;
    this.state = state; this.api = api;
    if (account !== this.account || runtime !== this.runtime) {
      this.generation++;
      this.account = account; this.runtime = runtime;
      this.operations.clear(); this.items = []; this.paused.clear();
      if (account) {
        try {
          const raw = this.storage?.getItem(key(account));
          const saved: unknown = raw ? JSON.parse(raw) : [];
          if (!Array.isArray(saved) || saved.length > MAX_MESSAGES) throw new Error("invalid queue");
          for (const value of saved) {
            if (!value || typeof value !== "object" || typeof value.id !== "string" || !value.id || value.id.length > 128
              || typeof value.threadId !== "string" || !value.threadId || value.threadId.length > 128
              || typeof value.text !== "string" || !value.text.trim() || new TextEncoder().encode(value.text).length > MAX_BYTES
              || typeof value.webEnabled !== "boolean" || this.items.some((item) => item.id === value.id)
              || !Number.isSafeInteger(value.agentRevision ?? 0) || (value.agentRevision ?? 0) < 0) throw new Error("invalid queue");
            this.items.push({ id: value.id, threadId: value.threadId, text: value.text, webEnabled: value.webEnabled,
              agentRevision: value.agentRevision ?? 0,
              status: "queued", error: "Restored after reconnect. Review and send when ready." });
            this.paused.add(value.threadId);
          }
        } catch { this.items = []; this.reportError("The saved message queue could not be read. Nothing was sent."); }
      }
      this.changed();
    }
    if (!account || !api) return;
    const remaining = this.items.filter((item) => !state.sessions[item.threadId]?.timeline.some((row) => row.kind === "user" && row.clientItemId === item.id));
    if (remaining.length !== this.items.length) { this.items = remaining; this.save(); this.changed(); }
    this.pump();
  }

  enqueue(threadId: string, text: string, webEnabled: boolean): string {
    if (!this.account || !this.api || !this.state?.connected) throw new Error("Connect to Axiom before sending.");
    if (this.items.length >= MAX_MESSAGES) throw new Error("The message queue is full. Send or remove a queued message first.");
    if (!text.trim() || new TextEncoder().encode(text).length > MAX_BYTES) throw new Error("Queued messages must contain text and be no larger than 64 KB.");
    const item: QueuedMessage = { id: crypto.randomUUID(), threadId, text, webEnabled,
      agentRevision: this.state.sessions[threadId]?.desktopAgent?.revision ?? 0, status: "queued" };
    this.items.push(item);
    try { this.persist(); } catch { this.items.pop(); throw new Error("The message could not be saved to the queue. Your draft was kept."); }
    this.changed(); this.pump(); return item.id;
  }
  remove(id: string): void {
    const item = this.items.find((item) => item.id === id);
    if (!item || item.status !== "queued") return;
    const previous = this.items;
    this.items = this.items.filter((item) => item.id !== id);
    if (!this.save()) this.items = previous;
    this.changed();
  }
  forget(threadId: string): void {
    this.items = this.items.filter((item) => item.threadId !== threadId);
    this.paused.delete(threadId); this.save(); this.changed();
  }
  pause(threadId: string): void {
    this.paused.add(threadId); this.changed();
  }
  sendNow(id: string): void {
    const item = this.items.find((item) => item.id === id);
    if (!item || item.status !== "queued") return;
    const session = this.state?.sessions[item.threadId];
    if (!session || !this.state?.connected || !this.account) return;
    const steering = session.running;
    if (steering && (!session.activeTurnId || [...this.operations.values()].some((op) => op.threadId === item.threadId && op.steering))) return;
    if (!steering && [...this.operations.values()].some((op) => op.threadId === item.threadId)) return;
    this.paused.delete(item.threadId);
    void this.dispatch(item, steering ? session.activeTurnId! : null);
  }
  private pump(): void {
    if (!this.account || !this.api || !this.state?.connected) return;
    const seen = new Set<string>();
    for (const item of this.items) {
      if (seen.has(item.threadId)) continue;
      seen.add(item.threadId);
      const session = this.state.sessions[item.threadId];
      if (item.status === "queued" && session && !session.running && !this.paused.has(item.threadId)
        && ![...this.operations.values()].some((op) => op.threadId === item.threadId)) void this.dispatch(item, null);
    }
  }
  private async dispatch(item: QueuedMessage, turnId: string | null): Promise<void> {
    const api = this.api;
    if (!api || !this.account) return;
    if (item.agentRevision !== (this.state?.sessions[item.threadId]?.desktopAgent?.revision ?? 0)) {
      item.error = "Agent settings changed. Remove this queued message and send it again with the settings you want.";
      this.paused.add(item.threadId); this.save(); this.changed(); return;
    }
    const generation = this.generation;
    item.status = turnId ? "steering" : "sending"; delete item.error;
    if (!this.save()) { item.status = "queued"; this.paused.add(item.threadId); this.changed(); return; }
    this.operations.set(item.id, { threadId: item.threadId, steering: turnId !== null });
    this.changed();
    try {
      if (turnId) {
        if (typeof api.steer !== "function") throw new Error("Restart Axiom to enable steering. Your message is still queued.");
        await api.steer({ threadId: item.threadId, expectedTurnId: turnId, clientItemId: item.id, text: item.text, webEnabled: item.webEnabled, agentRevision: item.agentRevision });
      } else {
        const result = await api.promptWithWebConsent(item.threadId, item.text, item.id, item.webEnabled, item.agentRevision);
        if (generation !== this.generation) return;
        this.sync(await api.getState(), api);
        if (generation !== this.generation) return;
        if (result.stopReason !== "end_turn") this.paused.add(item.threadId);
      }
      if (generation !== this.generation) return;
      // Steer resolves only after native persistence. Normal prompts are
      // removed by their echoed client ID, never merely by end_turn (e.g. auth).
      if (turnId) this.items = this.items.filter((queued) => queued.id !== item.id);
      else if (this.items.some((queued) => queued.id === item.id)) {
        item.status = "queued"; item.error = "Delivery was not confirmed. Check the thread before retrying.";
        this.paused.add(item.threadId);
      }
    } catch {
      if (generation !== this.generation) return;
      if (this.items.some((queued) => queued.id === item.id)) {
        item.status = "queued"; item.error = "Not confirmed. Your message is still queued; check the thread before retrying.";
      }
      this.paused.add(item.threadId);
    } finally {
      if (generation === this.generation) {
        this.operations.delete(item.id);
        const session = this.state?.sessions[item.threadId];
        if (session?.timeline.at(-1)?.kind === "error") this.paused.add(item.threadId);
        if (!this.save()) this.paused.add(item.threadId);
        this.changed(); this.pump();
      }
    }
  }
}

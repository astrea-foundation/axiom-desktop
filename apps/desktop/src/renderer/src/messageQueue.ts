import type { ClientState, PromptResult, SteerTurnRequest, SteerTurnResponse } from "@axiom/axiom-acp-client";
import { promptBytes, validatePrompt, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";
import { localPayloads, type PayloadStore } from "./localPayloads";
import type { ConsentStorage } from "./webConsent";

export interface QueuedMessage {
  id: string;
  threadId: string;
  text: string;
  attachments?: PromptAttachment[];
  payloadId?: string;
  webEnabled: boolean;
  agentRevision: number;
  status: "queued" | "sending" | "steering";
  preparing?: boolean;
  error?: string;
}
export interface QueueApi {
  getState(): Promise<ClientState>;
  promptWithWebConsent(thread: string, text: string, id: string, web: boolean, agentRevision: number): Promise<PromptResult>;
  promptWithAttachments?(thread: string, text: string, id: string, web: boolean, agentRevision: number, attachments: PromptAttachment[]): Promise<PromptResult>;
  steer(request: SteerTurnRequest): Promise<SteerTurnResponse>;
}
const MAX_MESSAGES = 32;
const MAX_QUEUE_BYTES = 64 * 1024 * 1024;
const key = (account: string) => `axiom.message-queue.v1:${encodeURIComponent(account)}`;

/** On-device outbox. A native transcript acknowledgement, not a UI timeout,
 * removes a message. No background replay after account/runtime changes. */
export class MessageQueue {
  private restoring: Promise<void> = Promise.resolve();
  private loading = false;
  private account: string | null = null;
  private runtime: string | null = null;
  private generation = 0;
  private state: ClientState | null = null;
  private api: QueueApi | null = null;
  private items: QueuedMessage[] = [];
  private paused = new Set<string>();
  private freshSends = new Set<string>();
  private operations = new Map<string, { threadId: string; steering: boolean }>();

  constructor(private storage: ConsentStorage | null, private changed: () => void, private reportError: (message: string) => void, private payloads: PayloadStore = localPayloads) {}

  list(threadId: string | null, accountId = this.account): QueuedMessage[] {
    if (accountId !== this.account) return [];
    return this.items.filter((item) => !item.preparing && item.threadId === threadId).map((item) => ({ ...item }));
  }
  preparations(accountId: string | null): QueuedMessage[] {
    return accountId === this.account ? this.items.filter((item) => item.preparing).map((item) => ({ ...item })) : [];
  }
  isPaused(threadId: string): boolean { return this.paused.has(threadId); }
  dispose(): void {
    this.generation++; this.account = null; this.runtime = null; this.api = null; this.state = null;
    this.operations.clear();
  }
  private persist(): void {
    if (!this.storage || !this.account) throw new Error("The message queue could not be saved. No new message was sent.");
    this.storage.setItem(key(this.account), JSON.stringify(this.items.map((item) => item.payloadId ? { ...item, text: "", attachments: undefined } : item)));
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
      this.operations.clear(); this.items = []; this.paused.clear(); this.freshSends.clear();
      if (account) {
        try {
          const raw = this.storage?.getItem(key(account));
          const saved: unknown = raw ? JSON.parse(raw) : [];
          if (!Array.isArray(saved) || saved.length > MAX_MESSAGES) throw new Error("invalid queue");
          const generation = this.generation;
          const restore = (value: QueuedMessage) => {
            if (!value || typeof value !== "object" || typeof value.id !== "string" || !value.id || value.id.length > 128
              || typeof value.threadId !== "string" || !value.threadId || value.threadId.length > 128
              || typeof value.webEnabled !== "boolean" || this.items.some((item) => item.id === value.id)
              || !Number.isSafeInteger(value.agentRevision ?? 0) || (value.agentRevision ?? 0) < 0) throw new Error("invalid queue");
            validatePrompt(value.text, value.attachments ?? []);
            if (this.items.reduce((sum, item) => sum + promptBytes(item.text, item.attachments ?? []), promptBytes(value.text, value.attachments ?? [])) > MAX_QUEUE_BYTES) throw new Error("invalid queue size");
            this.items.push({ id: value.id, threadId: value.threadId, text: value.text, attachments: value.attachments,
              payloadId: value.payloadId, webEnabled: value.webEnabled, agentRevision: value.agentRevision ?? 0,
              preparing: value.preparing === true, status: "queued", error: "Restored after reconnect. Review and send when ready." });
            this.paused.add(value.threadId);
          };
          if (saved.some((item) => item?.payloadId)) {
            this.loading = true;
            this.restoring = (async () => {
              for (const value of saved) {
                if (value.payloadId) {
                  if (value.payloadId !== value.id) throw new Error("invalid payload reference");
                  const payload = await this.payloads.get(account, value.payloadId);
                  if (!payload) throw new Error("missing payload");
                  Object.assign(value, payload);
                }
                if (generation !== this.generation) return;
                restore(value);
              }
            })().catch(() => {
              if (generation !== this.generation) return;
              this.items = []; this.reportError("The saved message queue could not be read. Nothing was sent.");
            }).finally(() => {
              if (generation !== this.generation) return;
              this.loading = false; this.changed();
              if (this.state) this.sync(this.state, this.api);
            });
          } else {
            this.loading = false;
            for (const value of saved) restore(value);
          }
        } catch { this.items = []; this.reportError("The saved message queue could not be read. Nothing was sent."); }
      }
      this.changed();
    }
    if (!account || !api || this.loading) return;
    const remaining = this.items.filter((item) => !state.sessions[item.threadId]?.timeline.some((row) => row.kind === "user" && row.clientItemId === item.id));
    if (remaining.length !== this.items.length) {
      const removed = this.items.filter((item) => !remaining.includes(item));
      this.items = remaining;
      if (this.save()) for (const item of removed) this.cleanup(item, account);
      this.changed();
    }
    this.pump();
  }

  enqueue(threadId: string, text: string, webEnabled: boolean): string {
    const id = this.reserve(threadId, text, webEnabled);
    this.ready(id, threadId);
    return id;
  }
  async reserveDurable(threadId: string | null, text: string, webEnabled: boolean, attachments: PromptAttachment[] = []): Promise<string> {
    const generation = this.generation;
    await this.restoring;
    if (generation !== this.generation || !this.account) throw new Error("The account or connection changed. Your draft was kept.");
    validatePrompt(text, attachments);
    const account = this.account;
    const id = crypto.randomUUID();
    await this.payloads.put(account, id, { text, attachments });
    try {
      if (generation !== this.generation) throw new Error("The account or connection changed. Your draft was kept.");
      return this.reserve(threadId, text, webEnabled, attachments, id);
    } catch (error) { await this.payloads.remove(account, id).catch(() => undefined); throw error; }
  }
  private cleanup(item: QueuedMessage, account = this.account): void {
    if (account && item.payloadId) void this.payloads.remove(account, item.payloadId).catch(() => undefined);
  }
  reserve(threadId: string | null, text: string, webEnabled: boolean, attachments: PromptAttachment[] = [], payloadId?: string): string {
    if (!this.account || !this.api || !this.state?.connected) throw new Error("Connect to Axiom before sending.");
    if (this.items.length >= MAX_MESSAGES) throw new Error("The message queue is full. Send or remove a queued message first.");
    if (this.loading) throw new Error("Wait for your saved messages to load.");
    validatePrompt(text, attachments);
    if (this.items.reduce((sum, item) => sum + promptBytes(item.text, item.attachments ?? []), promptBytes(text, attachments)) > MAX_QUEUE_BYTES) throw new Error("The local queue is full (64 MiB). Send or remove a queued message first.");
    const id = payloadId ?? crypto.randomUUID();
    const item: QueuedMessage = { id, threadId: threadId ?? `pending:${id}`, text, attachments, payloadId, webEnabled,
      agentRevision: threadId ? this.state.sessions[threadId]?.desktopAgent?.revision ?? 0 : 0,
      preparing: true, status: "sending" };
    this.items.push(item);
    try { this.persist(); } catch { this.items.pop(); throw new Error("The message could not be saved to the queue. Your draft was kept."); }
    this.changed();
    return item.id;
  }
  bindPreparation(id: string, threadId: string): void {
    const item = this.items.find((item) => item.id === id && item.preparing);
    if (!item) throw new Error("The account or connection changed. Your message remains saved for review.");
    item.threadId = threadId;
    this.persist(); this.changed();
  }
  failPreparation(id: string, error: string): void {
    const item = this.items.find((item) => item.id === id && item.preparing);
    if (!item) return;
    item.status = "queued"; item.error = error;
    this.save(); this.changed();
  }
  ready(id: string, threadId: string): void {
    const item = this.items.find((item) => item.id === id && item.preparing);
    if (!item || !this.state?.sessions[threadId]) throw new Error("The thread is not ready. Your message is saved for review.");
    const previous = { ...item };
    Object.assign(item, { threadId, preparing: false, status: "queued", agentRevision: this.state.sessions[threadId]?.desktopAgent?.revision ?? 0 });
    delete item.error;
    try { this.persist(); } catch (error) { Object.assign(item, previous); throw error; }
    // New intent survives Stop's asynchronous cleanup. It does not release
    // earlier held messages, and this permission never survives reconnect.
    if (this.paused.has(threadId)) this.freshSends.add(item.id);
    this.changed(); this.pump();
  }
  remove(id: string): void {
    const item = this.items.find((item) => item.id === id);
    if (!item || item.status !== "queued") return;
    const previous = this.items;
    this.items = this.items.filter((item) => item.id !== id);
    if (!this.save()) this.items = previous;
    else { this.freshSends.delete(id); this.cleanup(item); }
    this.changed();
  }
  forget(threadId: string): void {
    for (const item of this.items) if (item.threadId === threadId) this.freshSends.delete(item.id);
    const removed = this.items.filter((item) => item.threadId === threadId);
    this.items = this.items.filter((item) => item.threadId !== threadId);
    this.paused.delete(threadId);
    if (this.save()) for (const item of removed) this.cleanup(item);
    this.changed();
  }
  pause(threadId: string): void {
    for (const item of this.items) if (item.threadId === threadId) this.freshSends.delete(item.id);
    this.paused.add(threadId); this.changed();
  }
  sendNow(id: string): void {
    const item = this.items.find((item) => item.id === id);
    if (!item || item.preparing || item.status !== "queued") return;
    const session = this.state?.sessions[item.threadId];
    if (!session || !this.state?.connected || !this.account) return;
    const steering = session.running;
    if (steering && ((item.attachments?.length ?? 0) > 0 || new TextEncoder().encode(item.text).length > 64 * 1024)) {
      item.error = "This message will send as a new turn after the current reply.";
      this.freshSends.add(item.id); this.save(); this.changed(); return;
    }
    if (steering && (!session.activeTurnId || [...this.operations.values()].some((op) => op.threadId === item.threadId && op.steering))) return;
    if (!steering && [...this.operations.values()].some((op) => op.threadId === item.threadId)) return;
    void this.dispatch(item, steering ? session.activeTurnId! : null);
  }
  private pump(): void {
    if (!this.account || !this.api || !this.state?.connected || this.loading) return;
    const seen = new Set<string>();
    for (const item of this.items) {
      if (item.preparing) continue;
      if (this.paused.has(item.threadId) && !this.freshSends.has(item.id)) continue;
      if (seen.has(item.threadId)) continue;
      seen.add(item.threadId);
      const session = this.state.sessions[item.threadId];
      if (item.status === "queued" && session && !session.running
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
    this.freshSends.delete(item.id);
    item.status = turnId ? "steering" : "sending"; delete item.error;
    if (!this.save()) { item.status = "queued"; this.paused.add(item.threadId); this.changed(); return; }
    this.operations.set(item.id, { threadId: item.threadId, steering: turnId !== null });
    this.changed();
    try {
      if (turnId) {
        if (typeof api.steer !== "function") throw new Error("Restart Axiom to enable steering. Your message is still queued.");
        await api.steer({ threadId: item.threadId, expectedTurnId: turnId, clientItemId: item.id, text: item.text, webEnabled: item.webEnabled, agentRevision: item.agentRevision });
      } else {
        if (item.attachments?.length && !api.promptWithAttachments) throw new Error("Restart Axiom to enable attachments.");
        const result = item.attachments?.length
          ? await api.promptWithAttachments!(item.threadId, item.text, item.id, item.webEnabled, item.agentRevision, item.attachments)
          : await api.promptWithWebConsent(item.threadId, item.text, item.id, item.webEnabled, item.agentRevision);
        if (generation !== this.generation) return;
        this.sync(await api.getState(), api);
        if (generation !== this.generation) return;
        if (result.stopReason !== "end_turn") this.paused.add(item.threadId);
      }
      if (generation !== this.generation) return;
      // Steer resolves only after native persistence. Normal prompts are
      // removed by their echoed client ID, never merely by end_turn (e.g. auth).
      if (turnId) { this.items = this.items.filter((queued) => queued.id !== item.id); if (this.save()) this.cleanup(item); }
      else if (this.items.some((queued) => queued.id === item.id)) {
        item.status = "queued"; item.error = "Delivery was not confirmed. Check the thread before retrying.";
        this.paused.add(item.threadId);
      }
    } catch (error) {
      if (generation !== this.generation) return;
      if (this.items.some((queued) => queued.id === item.id)) {
        item.status = "queued"; item.error = `${error instanceof Error ? error.message + " " : ""}Not confirmed. Your message is still queued; check the thread before retrying.`;
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

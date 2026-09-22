import type { ClientState, PromptResult } from "@axiom/axiom-acp-client";
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
  status: "queued" | "sending" | "stopping";
  preparing?: boolean;
  error?: string;
}
export interface QueueApi {
  getState(): Promise<ClientState>;
  promptWithWebConsent(thread: string, text: string, id: string, web: boolean, agentRevision: number): Promise<PromptResult>;
  promptWithAttachments?(thread: string, text: string, id: string, web: boolean, agentRevision: number, attachments: PromptAttachment[]): Promise<PromptResult>;
  cancel(threadId: string): Promise<void>;
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
  private operations = new Map<string, string>();
  private interruptions = new Map<string, { id: string | null; ready: boolean }>();

  constructor(private storage: ConsentStorage | null, private changed: () => void, private reportError: (message: string) => void, private payloads: PayloadStore = localPayloads) {}

  list(threadId: string | null, accountId = this.account): QueuedMessage[] {
    if (accountId !== this.account) return [];
    return this.items.filter((item) => !item.preparing && item.threadId === threadId).map((item) => ({ ...item }));
  }
  preparations(accountId: string | null): QueuedMessage[] {
    return accountId === this.account ? this.items.filter((item) => item.preparing).map((item) => ({ ...item })) : [];
  }
  isPaused(threadId: string): boolean { return this.paused.has(threadId); }
  isStopping(threadId: string): boolean { return this.interruptions.has(threadId); }
  dispose(): void {
    this.generation++; this.account = null; this.runtime = null; this.api = null; this.state = null;
    this.operations.clear(); this.interruptions.clear();
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
      this.operations.clear(); this.interruptions.clear(); this.items = []; this.paused.clear(); this.freshSends.clear();
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
    if (!item || item.status === "sending") return;
    const previous = this.items;
    this.items = this.items.filter((item) => item.id !== id);
    if (!this.save()) { this.items = previous; this.pause(item.threadId); }
    else {
      const interruption = this.interruptions.get(item.threadId);
      if (interruption?.id === id) { interruption.id = null; this.paused.add(item.threadId); }
      this.freshSends.delete(id); this.cleanup(item);
    }
    this.changed();
  }
  forget(threadId: string): void {
    for (const item of this.items) if (item.threadId === threadId) this.freshSends.delete(item.id);
    const removed = this.items.filter((item) => item.threadId === threadId);
    this.items = this.items.filter((item) => item.threadId !== threadId);
    this.paused.delete(threadId);
    this.interruptions.delete(threadId);
    if (this.save()) for (const item of removed) this.cleanup(item);
    this.changed();
  }
  pause(threadId: string): void {
    for (const item of this.items) if (item.threadId === threadId) this.freshSends.delete(item.id);
    const interruption = this.interruptions.get(threadId);
    if (interruption) {
      const item = this.items.find((item) => item.id === interruption.id);
      if (item) item.status = "queued";
      interruption.id = null;
      this.save();
    }
    this.paused.add(threadId); this.changed();
  }
  sendNow(id: string): void {
    const item = this.items.find((item) => item.id === id);
    if (!item || item.preparing || item.status !== "queued") return;
    const session = this.state?.sessions[item.threadId];
    const api = this.api;
    if (!session || !this.state?.connected || !this.account || !api || this.interruptions.has(item.threadId)) return;
    if (!this.checkAgentRevision(item)) return;
    if (!session.running && !this.hasOperation(item.threadId)) { void this.dispatch(item); return; }
    this.freshSends.delete(item.id);
    item.status = "stopping"; delete item.error;
    if (!this.save()) { item.status = "queued"; this.paused.add(item.threadId); this.changed(); return; }
    const interruption = { id: item.id as string | null, ready: false };
    this.interruptions.set(item.threadId, interruption);
    this.changed();
    void this.interrupt(item.threadId, interruption, api);
  }
  private async interrupt(threadId: string, interruption: { id: string | null; ready: boolean }, api: QueueApi): Promise<void> {
    const generation = this.generation;
    const current = () => generation === this.generation && this.interruptions.get(threadId) === interruption;
    try {
      await api.cancel(threadId);
      if (!current()) return;
      const state = await api.getState();
      if (!current()) return;
      // Cancellation is a notification, not a completion acknowledgement.
      // Keep this thread gated until native state and the old prompt settle.
      interruption.ready = true;
      this.sync(state, api);
    } catch {
      if (!current()) return;
      const item = this.items.find((item) => item.id === interruption.id);
      if (item) { item.status = "queued"; item.error = "Couldn’t stop the reply. Try again."; }
      this.interruptions.delete(threadId);
      this.paused.add(threadId); this.save(); this.changed();
    }
  }
  private hasOperation(threadId: string): boolean {
    return [...this.operations.values()].includes(threadId);
  }
  private pump(): void {
    if (!this.account || !this.api || !this.state?.connected || this.loading) return;
    for (const [threadId, interruption] of this.interruptions) {
      const session = this.state.sessions[threadId];
      if (!interruption.ready || !session || session.running || this.hasOperation(threadId)) continue;
      this.interruptions.delete(threadId);
      const item = this.items.find((item) => item.id === interruption.id);
      if (item) void this.dispatch(item);
      else this.changed();
    }
    const seen = new Set<string>();
    for (const item of this.items) {
      if (item.preparing || this.interruptions.has(item.threadId)) continue;
      if (this.paused.has(item.threadId) && !this.freshSends.has(item.id)) continue;
      if (seen.has(item.threadId)) continue;
      seen.add(item.threadId);
      const session = this.state.sessions[item.threadId];
      if (item.status === "queued" && session && !session.running
        && !this.hasOperation(item.threadId)) void this.dispatch(item);
    }
  }
  private checkAgentRevision(item: QueuedMessage): boolean {
    if (item.agentRevision !== (this.state?.sessions[item.threadId]?.desktopAgent?.revision ?? 0)) {
      item.status = "queued";
      item.error = "Agent settings changed. Remove this queued message and send it again with the settings you want.";
      this.paused.add(item.threadId); this.save(); this.changed(); return false;
    }
    return true;
  }
  private async dispatch(item: QueuedMessage): Promise<void> {
    const api = this.api;
    if (!api || !this.account || !this.checkAgentRevision(item)) return;
    const generation = this.generation;
    this.freshSends.delete(item.id);
    item.status = "sending"; delete item.error;
    if (!this.save()) { item.status = "queued"; this.paused.add(item.threadId); this.changed(); return; }
    this.operations.set(item.id, item.threadId);
    this.changed();
    try {
      if (item.attachments?.length && !api.promptWithAttachments) throw new Error("Restart Axiom to enable attachments.");
      const result = item.attachments?.length
        ? await api.promptWithAttachments!(item.threadId, item.text, item.id, item.webEnabled, item.agentRevision, item.attachments)
        : await api.promptWithWebConsent(item.threadId, item.text, item.id, item.webEnabled, item.agentRevision);
      if (generation !== this.generation) return;
      const state = await api.getState();
      if (generation !== this.generation) return;
      this.sync(state, api);
      if (generation !== this.generation) return;
      if (result.stopReason !== "end_turn" && !this.interruptions.has(item.threadId)) this.paused.add(item.threadId);
      // Only an echoed client ID confirms delivery, never end_turn alone.
      if (this.items.some((queued) => queued.id === item.id)) {
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

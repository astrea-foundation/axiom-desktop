import type { PayloadStore, PromptPayload } from "@axiom/chat-core/localPayloads";
import type { QueueStore, QueuedMessage } from "@axiom/chat-core/messageQueue";
import { legacyPromptPayload, localEnvelope, promptPayload, queueItems, type LocalEnvelope } from "../../shared/local-payload";

export interface NativeLocalApi {
  localPayload(request: { accountId: string; id: string; action: { kind: "get" | "remove" } | { kind: "put"; value: LocalEnvelope } | { kind: "import_legacy"; value: LocalEnvelope; preview?: boolean } }): Promise<{ value?: unknown; omittedMedia?: number | null }>;
}
export interface LegacyPayloadStore extends PayloadStore {
  entries(account: string): Promise<Array<[string, PromptPayload]>>;
  removeIfUnchanged(account: string, id: string, expected: PromptPayload): Promise<boolean>;
}
export type LegacyStorage = Pick<Storage, "getItem" | "removeItem" | "key" | "length">;
const OMITTED_REVIEW = "Some damaged legacy media was omitted. Review this message before sending.";
const QUEUE_ID = "axiom.message-queue.v1";
const queueKey = (account: string) => `${QUEUE_ID}:${encodeURIComponent(account)}`;
const draftPrefix = (account: string) => `axiom.draft.v1:${encodeURIComponent(account)}:`;
function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${stableJson((value as Record<string, unknown>)[key])}`).join(",")}}`;
  return JSON.stringify(value) ?? "undefined";
}
const changedAccount = () => new Error("The account or connection changed. Your local data was kept.");

/** Desktop uses only native encrypted persistence. Browser stores are read once
 * during migration and deleted only after native commits acknowledge them. */
export class NativePersistence implements PayloadStore, QueueStore {
  private context = "";
  private account: string | null = null;
  private epoch = 0;
  private serial = new Map<string, Promise<unknown>>();
  private migrated = new Set<string>();
  private anonymous = new Map<string, PromptPayload>();

  constructor(private api: () => NativeLocalApi | undefined, private legacy: LegacyPayloadStore, private storage: () => LegacyStorage | null) {}

  setContext(account: string | null, runtime: string | null, revision: number): void {
    const context = JSON.stringify([account, runtime, revision]);
    if (context === this.context) return;
    this.context = context; this.account = account; this.epoch++;
    // A new native store/runtime must acknowledge migration independently.
    this.migrated.clear();
  }

  private async run<T>(account: string, operation: (native: NativeLocalApi) => Promise<T>): Promise<T> {
    const epoch = this.epoch;
    const check = () => { if (epoch !== this.epoch || account !== this.account) throw changedAccount(); };
    check();
    const work = (this.serial.get(account) ?? Promise.resolve()).catch(() => undefined).then(async () => {
      check();
      const api = this.api();
      if (!api) throw new Error("Encrypted local storage is unavailable. Keep Axiom open and restart with the updated native app.");
      const native: NativeLocalApi = { localPayload: async (request) => {
        check();
        const result = await api.localPayload(request);
        check();
        return result;
      } };
      await this.migrate(account, native);
      check();
      return operation(native);
    });
    this.serial.set(account, work);
    try { return await work; }
    finally { if (this.serial.get(account) === work) this.serial.delete(account); }
  }

  private async read(native: NativeLocalApi, accountId: string, id: string): Promise<LocalEnvelope | undefined> {
    const result = await native.localPayload({ accountId, id, action: { kind: "get" } });
    return result.value == null ? undefined : localEnvelope(result.value);
  }
  private async write(native: NativeLocalApi, accountId: string, id: string, value: LocalEnvelope): Promise<void> {
    await native.localPayload({ accountId, id, action: { kind: "put", value: localEnvelope(value) } });
  }

  private async digestId(prefix: string, value: unknown): Promise<string> {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(stableJson(value)));
    return `${prefix}:${Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("")}`;
  }
  private async importEnvelope(native: NativeLocalApi, accountId: string, id: string, source: LocalEnvelope, preview = false): Promise<{ value: LocalEnvelope; omitted: number }> {
    const response = await native.localPayload({ accountId, id, action: { kind: "import_legacy", value: localEnvelope(source, true), preview } });
    if (response.value == null || !Number.isSafeInteger(response.omittedMedia) || response.omittedMedia! < 0) throw new Error("The legacy import was not acknowledged. Your original local data was kept.");
    const value = localEnvelope(response.value);
    return { value, omitted: response.omittedMedia! };
  }
  private async importPrompt(native: NativeLocalApi, accountId: string, id: string, payload: PromptPayload, preview = false): Promise<{ payload: PromptPayload; omitted: number }> {
    const { value, omitted } = await this.importEnvelope(native, accountId, id, { version: 1, kind: "prompt", payload }, preview);
    if (value.kind !== "prompt") throw new Error("Invalid normalized draft.");
    return { payload: value.payload, omitted };
  }
  private async preserveRecovery(native: NativeLocalApi, account: string, prefix: string, id: string, payload: PromptPayload): Promise<string> {
    const recoveryId = await this.digestId(prefix, [id, payload]);
    const normalized = await this.importPrompt(native, account, recoveryId, payload);
    const saved = await this.read(native, account, recoveryId);
    // Compare with the acknowledged normalized source, including after lost ACK.
    if (saved?.kind !== "prompt" || stableJson(saved.payload) !== stableJson(normalized.payload)) throw new Error("A recovered draft could not be verified. Nothing was removed.");
    return recoveryId;
  }

  private async migrate(account: string, native: NativeLocalApi): Promise<void> {
    if (this.migrated.has(account)) return;
    const storage = this.storage();
    if (!storage) throw new Error("Legacy local storage could not be checked. Nothing was removed.");
    const entries = await this.legacy.entries(account);
    const anonymousEntries = await this.legacy.entries("anonymous");
    const payloads = new Map<string, PromptPayload>();
    const anonymous = new Map<string, PromptPayload>();
    const receipts: string[] = [];
    const needsImport = async (source: unknown) => {
      const id = await this.digestId("legacy-import", source);
      const saved = await this.read(native, account, id);
      if (saved && (saved.kind !== "prompt" || saved.payload.text || saved.payload.attachments.length)) throw new Error("Invalid migration acknowledgement.");
      if (saved) return false;
      receipts.push(id);
      return true;
    };
    for (const [owner, values, target] of [[account, entries, payloads], ["anonymous", anonymousEntries, anonymous]] as const) {
      for (const [id, value] of values) {
        if (await needsImport(["payload", owner, id, value])) {
          const payload = legacyPromptPayload(value);
          // Archive the exact IDB source before a newer text mirror is combined.
          const composer = id.startsWith(draftPrefix("anonymous")) ? decodeURIComponent(id.slice(draftPrefix("anonymous").length))
            : id.startsWith("draft-attachments:") ? id.slice("draft-attachments:".length) : id;
          const destination = owner === account ? id : payload.attachments.length
            ? `draft-attachments:${composer}` : draftPrefix(account) + encodeURIComponent(composer);
          await this.importPrompt(native, account, destination, payload, true);
          target.set(id, payload);
        }
      }
    }
    const textKeys = new Map<string, string>();
    for (let index = 0; index < storage.length; index++) {
      const id = storage.key(index);
      if (id?.startsWith(draftPrefix(account)) || id?.startsWith(draftPrefix("anonymous"))) {
        const text = storage.getItem(id);
        if (text !== null) {
          // The synchronous mirror can be newer than the old debounced IDB copy.
          textKeys.set(id, text);
          if (await needsImport(["text", id, text])) {
            const target = id.startsWith(draftPrefix(account)) ? payloads : anonymous;
            target.set(id, legacyPromptPayload({ text, attachments: target.get(id)?.attachments ?? [] }));
          }
        }
      }
    }
    const legacyQueueText = storage.getItem(queueKey(account));
    const oldQueue = legacyQueueText === null ? [] : queueItems(JSON.parse(legacyQueueText), true);
    const savedQueue = await this.read(native, account, QUEUE_ID);
    if (savedQueue && savedQueue.kind !== "queue") throw new Error("Invalid native queue record.");
    const recovered: QueuedMessage[] = [];
    const prepareRecovery = (id: string, error: string) => recovered.push({
      id, threadId: `pending:${id}`, payloadId: id, text: "", webEnabled: false,
      agentRevision: 0, status: "queued", preparing: true, error,
    });
    const imported = new Map<string, { id: string; omitted: number }>();
    const queuedPayloads = new Set(oldQueue.map(item => item.payloadId ?? item.id));
    for (const [id, payload] of payloads) {
      const normalized = await this.importPrompt(native, account, id, payload);
      const saved = await this.read(native, account, id);
      if (saved?.kind !== "prompt") throw new Error("Invalid native draft record.");
      const conflict = stableJson(saved.payload) !== stableJson(normalized.payload);
      let importedId = id;
      if (conflict || (normalized.omitted && !queuedPayloads.has(id))) {
        importedId = await this.preserveRecovery(native, account, "recovered-legacy", id, payload);
        if (!queuedPayloads.has(id)) prepareRecovery(importedId, normalized.omitted ? OMITTED_REVIEW : "Recovered an earlier saved draft. Review it before sending.");
      }
      imported.set(id, { id: importedId, omitted: normalized.omitted });
    }
    {
      const composers = new Map<string, PromptPayload>();
      for (const [id, payload] of anonymous) {
        const composer = id.startsWith(draftPrefix("anonymous")) ? decodeURIComponent(id.slice(draftPrefix("anonymous").length))
          : id.startsWith("draft-attachments:") ? id.slice("draft-attachments:".length) : id;
        const previous = composers.get(composer) ?? { text: "", attachments: [] };
        composers.set(composer, legacyPromptPayload({
          text: [previous.text, payload.text].filter(Boolean).join("\n\n"),
          attachments: [...previous.attachments, ...payload.attachments],
        }));
      }
      for (const [composer, payload] of composers) {
        if (!payload.text && !payload.attachments.length) continue;
        const textId = draftPrefix(account) + encodeURIComponent(composer);
        const attachmentId = `draft-attachments:${composer}`;
        const normalized = await this.importPrompt(native, account, attachmentId, payload, true);
        const text = await this.read(native, account, textId);
        const attachments = await this.read(native, account, attachmentId);
        if ((text && text.kind !== "prompt") || (attachments && attachments.kind !== "prompt")) throw new Error("Invalid native draft record.");
        const wantedText = { text: normalized.payload.text, attachments: [] };
        const wantedAttachments = { text: "", attachments: normalized.payload.attachments };
        const occupied = (text?.kind === "prompt" && (text.payload.text || text.payload.attachments.length) && stableJson(text.payload) !== stableJson(wantedText))
          || (attachments?.kind === "prompt" && (attachments.payload.text || attachments.payload.attachments.length) && stableJson(attachments.payload) !== stableJson(wantedAttachments));
        if (occupied) {
          const id = await this.preserveRecovery(native, account, "recovered-anonymous", composer, payload);
          prepareRecovery(id, normalized.omitted ? OMITTED_REVIEW : "Recovered a draft from before sign-in. Review it before sending.");
        } else {
          await this.write(native, account, textId, { version: 1, kind: "prompt", payload: wantedText });
          await this.write(native, account, attachmentId, { version: 1, kind: "prompt", payload: wantedAttachments });
          if (normalized.omitted) {
            const id = await this.preserveRecovery(native, account, "recovered-anonymous", composer, payload);
            prepareRecovery(id, OMITTED_REVIEW);
          }
        }
      }
    }
    {
      const merged = new Map((savedQueue?.items ?? []).map(item => [item.id, item]));
      for (const item of recovered) if (!merged.has(item.id)) merged.set(item.id, item);
      for (const item of oldQueue) {
        // Queue IDs identify one submission even if its delivery status changed.
        // A receipt prevents an old browser snapshot replaying a delivered item.
        if (!await needsImport(["queue", item.id])) continue;
        // Archive a damaged item's complete original metadata too, before
        // splitting its input into a separately stored prompt. Bound each call
        // to one item so large legacy outboxes do not exceed ACP frame limits.
        await this.importEnvelope(native, account, QUEUE_ID, { version: 1, kind: "queue", items: [item] }, true);
        if (merged.has(item.id)) continue;
        let payloadId = item.payloadId ?? item.id;
        let omitted = 0;
        if (item.payloadId) {
          const source = imported.get(item.payloadId);
          payloadId = source?.id ?? item.payloadId;
          omitted = source?.omitted ?? 0;
          const payload = await this.read(native, account, payloadId);
          if (payload?.kind !== "prompt") throw new Error("A queued message's saved input is missing. Nothing was removed.");
        } else {
          const source = { text: item.text, attachments: item.attachments ?? [] };
          const normalized = await this.importPrompt(native, account, item.id, source);
          const saved = await this.read(native, account, item.id);
          if (saved?.kind !== "prompt") throw new Error("Invalid queued payload record.");
          if (stableJson(saved.payload) !== stableJson(normalized.payload)) payloadId = await this.preserveRecovery(native, account, "recovered-queue", item.id, source);
          omitted = normalized.omitted;
        }
        merged.set(item.id, { ...item, payloadId, text: "", attachments: undefined,
          ...(omitted ? { status: "queued", error: [item.error, OMITTED_REVIEW].filter(Boolean).join(" ") } : {}),
        });
      }
      await this.write(native, account, QUEUE_ID, { version: 1, kind: "queue", items: [...merged.values()], legacyImported: true });
    }
    // Acknowledge exact source versions only after their payloads and recoverable
    // queue entries commit. These encrypted, empty records survive item removal,
    // so retry neither replays delivered items nor drops changed legacy drafts.
    for (const id of receipts) await this.write(native, account, id, { version: 1, kind: "prompt", payload: { text: "", attachments: [] } });
    // Every payload and the queue metadata are now acknowledged by native.
    // Comparison preserves any legacy writer that changed a value during import.
    for (const [id, text] of textKeys) {
      if (storage.getItem(id) === text) storage.removeItem(id);
      else throw new Error("A legacy draft changed during migration. Keep Axiom open and retry.");
    }
    for (const [owner, values] of [[account, entries], ["anonymous", anonymousEntries]] as const) {
      for (const [id, payload] of values) {
        if (!await this.legacy.removeIfUnchanged(owner, id, payload)) throw new Error("A legacy attachment changed during migration. Keep Axiom open and retry.");
      }
    }
    if (storage.getItem(queueKey(account)) === legacyQueueText) storage.removeItem(queueKey(account));
    else throw new Error("The legacy queue changed during migration. Keep Axiom open and retry.");
    this.migrated.add(account);
  }

  async get(account: string, id: string): Promise<PromptPayload | undefined> {
    if (account === "anonymous") return this.anonymous.get(id);
    return this.run(account, async native => {
      const value = await this.read(native, account, id);
      if (value && value.kind !== "prompt") throw new Error("Invalid saved draft.");
      return value?.payload;
    });
  }
  async put(account: string, id: string, payload: PromptPayload): Promise<void> {
    payload = promptPayload(payload);
    if (account === "anonymous") { this.anonymous.set(id, payload); return; }
    await this.run(account, native => this.write(native, account, id, { version: 1, kind: "prompt", payload }));
  }
  async remove(account: string, id: string): Promise<void> {
    if (account === "anonymous") { this.anonymous.delete(id); return; }
    await this.run(account, async native => { await native.localPayload({ accountId: account, id, action: { kind: "remove" } }); });
  }
  async load(account: string): Promise<QueuedMessage[]> {
    return this.run(account, async native => {
      const value = await this.read(native, account, QUEUE_ID);
      if (value && value.kind !== "queue") throw new Error("Invalid saved queue.");
      return value?.items ?? [];
    });
  }
  async save(account: string, items: readonly QueuedMessage[]): Promise<void> {
    await this.run(account, native => this.write(native, account, QUEUE_ID, { version: 1, kind: "queue", items: [...items], legacyImported: true }));
  }
  async flush(): Promise<void> { await Promise.all([...this.serial.values()]); }
}

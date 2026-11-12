import { validatePrompt } from "@axiom/chat-core/attachmentValidation";
import type { PromptPayload } from "@axiom/chat-core/localPayloads";
import type { QueuedMessage } from "@axiom/chat-core/messageQueue";

export type LocalEnvelope =
  | { version: 1; kind: "prompt"; payload: PromptPayload }
  | { version: 1; kind: "queue"; items: QueuedMessage[]; legacyImported?: boolean };

export function promptPayload(value: unknown): PromptPayload {
  if (!value || typeof value !== "object") throw new Error("Invalid saved draft.");
  const payload = value as PromptPayload;
  if (typeof payload.text !== "string" || !Array.isArray(payload.attachments)) throw new Error("Invalid saved draft.");
  if (payload.text || payload.attachments.length) validatePrompt(payload.text, payload.attachments, true);
  return { text: payload.text, attachments: payload.attachments };
}

export function queueItems(value: unknown): QueuedMessage[] {
  if (!Array.isArray(value)) throw new Error("Invalid saved queue.");
  const ids = new Set<string>();
  for (const item of value as QueuedMessage[]) {
    if (!item || typeof item !== "object" || typeof item.id !== "string" || !item.id || item.id.length > 128
      || ids.has(item.id) || typeof item.threadId !== "string" || !item.threadId || item.threadId.length > 128
      || typeof item.webEnabled !== "boolean" || !["queued", "sending", "stopping"].includes(item.status)
      || !Number.isSafeInteger(item.agentRevision ?? 0) || (item.agentRevision ?? 0) < 0
      || (item.mcpRevision !== undefined && (!Number.isSafeInteger(item.mcpRevision) || item.mcpRevision < 0))
      || (item.payloadId !== undefined && (typeof item.payloadId !== "string" || !item.payloadId || item.payloadId.length > 2048 || /[\x00-\x1f\x7f]/.test(item.payloadId)))
      || (item.error !== undefined && typeof item.error !== "string")
      || (item.preparing !== undefined && typeof item.preparing !== "boolean")) throw new Error("Invalid saved queue.");
    const payload = promptPayload({ text: item.text, attachments: item.attachments ?? [] });
    if (!item.payloadId) validatePrompt(payload.text, payload.attachments, true);
    ids.add(item.id);
  }
  return value as QueuedMessage[];
}

export function localEnvelope(value: unknown): LocalEnvelope {
  if (!value || typeof value !== "object" || (value as LocalEnvelope).version !== 1) throw new Error("Invalid local data envelope.");
  const envelope = value as LocalEnvelope;
  if (envelope.kind === "prompt") return { version: 1, kind: "prompt", payload: promptPayload(envelope.payload) };
  if (envelope.kind === "queue") {
    if (envelope.legacyImported !== undefined && typeof envelope.legacyImported !== "boolean") throw new Error("Invalid migration acknowledgement.");
    return { version: 1, kind: "queue", items: queueItems(envelope.items), legacyImported: envelope.legacyImported };
  }
  throw new Error("Invalid local data kind.");
}

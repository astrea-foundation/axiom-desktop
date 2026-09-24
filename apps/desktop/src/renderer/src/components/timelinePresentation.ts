import type { ClientTimelineItem } from "@axiom/axiom-acp-client";
import type { Message } from "../types";

export type TimelinePresentationRow =
  | { kind: "message"; key: string; message: Message }
  | { kind: "tool"; key: string; item: ClientTimelineItem }
  | { kind: "activity"; key: string; item: ClientTimelineItem };

/** Revisions replace whole turns; a steering message is not a turn boundary. */
export function messageRevisionSources(items: ClientTimelineItem[]): Map<string, ClientTimelineItem> {
  const sources = new Map<string, ClientTimelineItem>();
  const turns = new Map<string, ClientTimelineItem>();
  let latest: ClientTimelineItem | undefined;
  for (const item of items) {
    if (item.kind === "user") {
      const metadata = (item.raw as { metadata?: { steering?: boolean } } | undefined)?.metadata;
      if (metadata?.steering || (item.turnId && turns.has(item.turnId))) continue;
      latest = item;
      if (item.turnId) turns.set(item.turnId, item);
      sources.set(item.id, item);
    } else if (item.kind === "assistant" || item.kind === "reasoning") {
      const source = item.turnId ? turns.get(item.turnId) : latest;
      if (source) sources.set(item.id, source);
    }
  }
  return sources;
}

function messageStatus(
  status: string | undefined,
  fallback: Message["status"] = "complete",
): Message["status"] {
  switch (status) {
    case "pending": return "pending";
    case "in_progress":
    case "streaming": return "streaming";
    case "failed": return "failed";
    case "interrupted": return "interrupted";
    case "cancelled": return "cancelled";
    case "completed": return "complete";
    default: return fallback;
  }
}

function terminalRank(status: Message["status"]): number {
  switch (status) {
    case "failed": return 6;
    case "interrupted": return 5;
    case "cancelled": return 4;
    case "streaming": return 3;
    case "pending": return 2;
    case "complete": return 1;
  }
}

function combinedStatus(
  reasoning: ClientTimelineItem,
  assistant: ClientTimelineItem,
): Message["status"] {
  const candidates = [
    messageStatus(reasoning.status),
    messageStatus(assistant.status, "streaming"),
  ];
  return candidates.reduce((selected, candidate) =>
    terminalRank(candidate) > terminalRank(selected) ? candidate : selected
  );
}

function messageFor(item: ClientTimelineItem, status = item.status): Message {
  if (item.kind === "reasoning") {
    return {
      id: item.id,
      role: "assistant",
      status: messageStatus(status),
      content: "",
      reasoningContent: item.text,
      terminalVerified: item.terminalVerified === true,
      finishReason: item.finishReason,
    };
  }
  return {
    id: item.id,
    role: item.kind === "user" ? "user" : "assistant",
    status: messageStatus(status, item.kind === "user" ? "complete" : "streaming"),
    content: item.text,
    terminalVerified: item.terminalVerified === true,
    finishReason: item.finishReason,
  };
}

function combinedAssistantMessage(
  reasoning: ClientTimelineItem,
  assistant: ClientTimelineItem,
): Message {
  return {
    id: assistant.id,
    role: "assistant",
    status: combinedStatus(reasoning, assistant),
    content: assistant.text,
    reasoningContent: reasoning.text,
    terminalVerified: assistant.terminalVerified === true,
    finishReason: assistant.finishReason,
  };
}

function sameTurn(left: ClientTimelineItem, right: ClientTimelineItem): boolean {
  return Boolean(left.turnId && right.turnId && left.turnId === right.turnId);
}

function emptyTerminalAssistant(item: ClientTimelineItem): boolean {
  return item.kind === "assistant"
    && !item.text
    && !["pending", "in_progress", "streaming"].includes(item.status ?? "");
}

function routineNotice(item: ClientTimelineItem): boolean {
  // Standard ACP presents connection notices as thought chunks. They are
  // operational status, not model reasoning. Failed connections remain visible.
  if (item.kind === "reasoning" && /^provider:\d+$/.test(item.id)) {
    return item.text.startsWith("[provider] ");
  }
  if (item.kind !== "activity" || !item.raw || typeof item.raw !== "object") return false;
  const raw = item.raw as Record<string, unknown>;
  if (raw.kind !== "notice" || !raw.metadata || typeof raw.metadata !== "object") return false;
  const code = (raw.metadata as Record<string, unknown>).code;
  // Keep these records in durable history; the composer owns their UI.
  return code === "provider_status" || code === "security_status" || code === "permission";
}

export function hasActiveResponse(rows: TimelinePresentationRow[]): boolean {
  // A trailing notice or tool row must not create another pending bubble.
  // Stop at the latest user message so an older turn cannot suppress it.
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const row = rows[index];
    if (row?.kind === "tool" && ["pending", "in_progress"].includes(row.item.tool?.status ?? "")) return true;
    if (row?.kind !== "message") continue;
    if (row.message.role === "user") return false;
    if (row.message.continues) continue;
    if (row.message.status === "pending" || row.message.status === "streaming") return true;
  }
  return false;
}

/**
 * Convert protocol timeline records to visual rows without inventing a
 * second transcript. Durable storage creates its assistant placeholder before
 * the optional reasoning item, while live ACP commonly delivers reasoning
 * first, so both adjacent orders are normalized by stable turn identity.
 */
export function presentTimeline(items: ClientTimelineItem[]): TimelinePresentationRow[] {
  // Filter before grouping so invisible status records cannot split a model's
  // reasoning and assistant output into two separate generation indicators.
  const timeline = items.filter((item) => !routineNotice(item));
  const rows: TimelinePresentationRow[] = [];
  for (let index = 0; index < timeline.length; index += 1) {
    const item = timeline[index];
    if (!item) continue;
    const next = timeline[index + 1];
    if (
      next
      && sameTurn(item, next)
      && item.kind === "reasoning"
      && next.kind === "assistant"
    ) {
      rows.push({
        kind: "message",
        key: next.id,
        message: combinedAssistantMessage(item, next),
      });
      index += 1;
    } else if (
      next
      && sameTurn(item, next)
      && item.kind === "assistant"
      && next.kind === "reasoning"
    ) {
      rows.push({
        kind: "message",
        key: item.id,
        message: combinedAssistantMessage(next, item),
      });
      index += 1;
    } else if (item.kind === "tool" && item.tool) {
      rows.push({ kind: "tool", key: item.id, item });
    } else if (item.kind === "user" || item.kind === "assistant" || item.kind === "reasoning") {
      if (emptyTerminalAssistant(item)) continue;
      const isLast = index === timeline.length - 1;
      const status = item.kind === "reasoning"
        && !isLast
        && item.status === "streaming"
        ? "completed"
        : item.status;
      rows.push({ kind: "message", key: item.id, message: messageFor(item, status) });
    } else {
      if (item.kind !== "error" && !item.text.trim()) continue;
      rows.push({ kind: "activity", key: item.id, item });
    }
  }
  // Intermediate segments keep their real verification/status; only their
  // presentation changes (no duplicate action bar or stale Generating row).
  const byId = new Map(timeline.map((item) => [item.id, item]));
  const followingTurns = new Set<string>();
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const row = rows[index];
    if (!row) continue;
    const turnId = byId.get(row.key)?.turnId;
    if (!turnId) continue;
    if (row.kind === "message" && row.message.role === "assistant") {
      row.message.continues = followingTurns.has(turnId);
      followingTurns.add(turnId);
    } else if (row.kind === "tool") {
      followingTurns.add(turnId);
    }
  }
  return rows.filter((row) => row.kind !== "message" || !row.message.continues
    || row.message.content || row.message.reasoningContent);
}

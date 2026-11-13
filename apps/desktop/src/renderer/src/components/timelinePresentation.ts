import type { ClientTimelineItem } from "@axiom/axiom-acp-client";
import type { Message } from "../types";

export type TimelinePresentationRow =
  | { kind: "message"; key: string; message: Message }
  | { kind: "tool"; key: string; item: ClientTimelineItem }
  | { kind: "activity"; key: string; item: ClientTimelineItem };

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
    };
  }
  return {
    id: item.id,
    role: item.kind === "user" ? "user" : "assistant",
    status: messageStatus(status, item.kind === "user" ? "complete" : "streaming"),
    content: item.text,
    terminalVerified: item.terminalVerified === true,
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

/**
 * Convert protocol timeline records to visual rows without inventing a
 * second transcript. Durable storage creates its assistant placeholder before
 * the optional reasoning item, while live ACP commonly delivers reasoning
 * first, so both adjacent orders are normalized by stable turn identity.
 */
export function presentTimeline(timeline: ClientTimelineItem[]): TimelinePresentationRow[] {
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
  return rows;
}

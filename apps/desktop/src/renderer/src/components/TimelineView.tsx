import { AlertTriangle, CheckCircle2, ListChecks } from "lucide-react";
import type { ClientSessionState, ClientTimelineItem } from "@axiom/axiom-acp-client";
import type { PendingUserMessage } from "../types";
import { MessageBubble } from "./MessageBubble";
import { ToolCallCard } from "./ToolCallCard";
import { InteractionCard } from "./InteractionCard";
import { hasActiveResponse, presentTimeline } from "./timelinePresentation";

function ActivityRow({ item }: { item: ClientTimelineItem }) {
  const error = item.kind === "error";
  return (
    <div className={`flex items-start gap-2 rounded-lg px-3 py-2 text-[12.5px] ${error ? "bg-[var(--color-danger-soft)] text-[var(--color-danger-strong)]" : "bg-[var(--wash-row)] text-[var(--color-text-tertiary)]"}`}>
      {error ? <AlertTriangle size={14} className="mt-0.5 shrink-0" /> : item.kind === "plan" ? <ListChecks size={14} className="mt-0.5 shrink-0" /> : <CheckCircle2 size={14} className="mt-0.5 shrink-0" />}
      <span>{item.text}</span>
    </div>
  );
}

export function TimelineView({
  session,
  pendingUserMessage,
}: {
  session: ClientSessionState;
  pendingUserMessage?: PendingUserMessage | null;
}) {
  const presentation = presentTimeline(session.timeline);
  const rows = presentation.map((row) => {
    if (row.kind === "message") {
      return <div key={row.key} data-timeline-kind={row.message.role}><MessageBubble message={row.message} /></div>;
    }
    if (row.kind === "tool" && row.item.tool) {
      return <div key={row.item.tool.callId} data-timeline-kind="tool"><ToolCallCard tool={row.item.tool} /></div>;
    }
    return <div key={row.key} data-timeline-kind="activity"><ActivityRow item={row.item} /></div>;
  });
  const showPendingUser = Boolean(
    pendingUserMessage
      && pendingUserMessage.sessionId === session.sessionId
      && !session.timeline.some((item) => item.clientItemId === pendingUserMessage.id),
  );
  if (showPendingUser && pendingUserMessage) {
    rows.push(
      <div key={pendingUserMessage.id} data-timeline-kind="user"><MessageBubble
        message={{
          id: pendingUserMessage.id,
          role: "user",
          status: "pending",
          content: pendingUserMessage.text,
        }}
      /></div>,
    );
  }
  const lastAlreadyShowsGeneration = !showPendingUser
    && hasActiveResponse(presentation);

  return (
    <div className="chat-timeline">
      {rows}
      {session.running && !lastAlreadyShowsGeneration ? (
        <div data-timeline-kind="assistant"><MessageBubble
          message={{ id: "active-generation", role: "assistant", status: "streaming", content: "" }}
        /></div>
      ) : null}
      {session.interactions.map((interaction) => (
        <div key={interaction.id} data-timeline-kind="interaction"><InteractionCard interaction={interaction} /></div>
      ))}
    </div>
  );
}

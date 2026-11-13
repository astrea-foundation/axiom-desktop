import { AlertTriangle, CheckCircle2, ListChecks } from "lucide-react";
import type { ClientSessionState, ClientTimelineItem } from "@axiom/axiom-acp-client";
import type { PendingUserMessage } from "../types";
import { MessageBubble } from "./MessageBubble";
import { ToolCallCard } from "./ToolCallCard";
import { InteractionCard } from "./InteractionCard";
import { presentTimeline } from "./timelinePresentation";

function ActivityRow({ item }: { item: ClientTimelineItem }) {
  const error = item.kind === "error";
  return (
    <div className={`ml-11 flex items-start gap-2 rounded-lg px-3 py-2 text-[12.5px] ${error ? "bg-red-500/10 text-red-300" : "bg-white/[0.025] text-[var(--color-text-tertiary)]"}`}>
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
  const rows = presentTimeline(session.timeline).map((row) => {
    if (row.kind === "message") {
      return <MessageBubble key={row.key} message={row.message} />;
    }
    if (row.kind === "tool" && row.item.tool) {
      return <ToolCallCard key={row.key} tool={row.item.tool} />;
    }
    return <ActivityRow key={row.key} item={row.item} />;
  });
  const last = session.timeline.at(-1);
  const showPendingUser = Boolean(
    pendingUserMessage
      && pendingUserMessage.sessionId === session.sessionId
      && !session.timeline.some((item) => item.clientItemId === pendingUserMessage.id),
  );
  if (showPendingUser && pendingUserMessage) {
    rows.push(
      <MessageBubble
        key={pendingUserMessage.id}
        message={{
          id: pendingUserMessage.id,
          role: "user",
          status: "pending",
          content: pendingUserMessage.text,
        }}
      />,
    );
  }
  const lastAlreadyShowsGeneration = !showPendingUser
    && (last?.kind === "assistant" || last?.kind === "reasoning");

  return (
    <>
      {rows}
      {session.running && !lastAlreadyShowsGeneration ? (
        <MessageBubble
          message={{ id: "active-generation", role: "assistant", status: "streaming", content: "" }}
        />
      ) : null}
      {session.interactions.map((interaction) => (
        <InteractionCard key={interaction.id} interaction={interaction} />
      ))}
    </>
  );
}

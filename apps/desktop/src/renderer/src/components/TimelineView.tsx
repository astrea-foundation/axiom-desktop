import { AlertTriangle, CheckCircle2, ListChecks } from "lucide-react";
import type { ClientSessionState, ClientTimelineItem } from "@axiom/axiom-acp-client";
import type { PendingUserMessage } from "../types";
import { replyErrorPresentation } from "../replyErrors";
import { AttachmentCards, StoredAttachments, attachmentSummaries } from "./Attachments";
import type { AttachmentPreviews } from "../attachmentPreviews";
import { MessageBubble } from "./MessageBubble";
import { ToolCallCard } from "./ToolCallCard";
import { InteractionCard } from "./InteractionCard";
import { DismissibleNotice } from "./DismissibleNotice";
import { hasActiveResponse, messageRevisionSources, presentTimeline } from "./timelinePresentation";

function ActivityRow({ item }: { item: ClientTimelineItem }) {
  const error = item.kind === "error";
  if (error && item.text.includes("Intel TDX environment is OutOfDate")) return <div role="status" data-timeline-kind="activity"
    className="flex items-start gap-2 rounded-lg bg-amber-500/10 px-3 py-2 text-[12.5px] text-[var(--color-warning)]">
    <AlertTriangle size={14} aria-hidden="true" className="mt-0.5 shrink-0" />
    <span>Generation paused because the provider’s TEE needs security updates.</span>
  </div>;
  const failure = error ? replyErrorPresentation(item.text) : null;
  if (failure) return (
    <DismissibleNotice data-timeline-kind="activity" role={failure.interrupted ? "status" : "alert"} className={`flex items-start gap-2 rounded-lg px-3 py-2 text-[12.5px] ${failure.interrupted ? "bg-[var(--wash-row)] text-[var(--color-text-secondary)]" : "bg-[var(--color-danger-soft)] text-[var(--color-danger-strong)]"}`}>
      <AlertTriangle size={14} aria-hidden="true" className="mt-0.5 shrink-0" />
      <div className="min-w-0">
        <p>{failure.message}</p>
        <details data-preserve-follow className="mt-1 text-[12px] text-[var(--color-text-tertiary)]">
          <summary className="w-fit cursor-pointer rounded-sm hover:text-[var(--color-text-secondary)]">Details</summary>
          <p className="mt-1 break-words">{item.text}</p>
        </details>
      </div>
    </DismissibleNotice>
  );
  if (error) return <DismissibleNotice data-timeline-kind="activity" className="flex items-start gap-2 rounded-lg bg-[var(--color-danger-soft)] px-3 py-2 text-[12.5px] text-[var(--color-danger-strong)]">
    <AlertTriangle size={14} aria-hidden="true" className="mt-0.5 shrink-0" />
    <span className="min-w-0 break-words">{item.text}</span>
  </DismissibleNotice>;
  return (
    <div data-timeline-kind="activity" className="flex items-start gap-2 rounded-lg bg-[var(--wash-row)] px-3 py-2 text-[12.5px] text-[var(--color-text-tertiary)]">
      {item.kind === "plan" ? <ListChecks size={14} className="mt-0.5 shrink-0" /> : <CheckCircle2 size={14} className="mt-0.5 shrink-0" />}
      <span>{item.text}</span>
    </div>
  );
}

export function TimelineView({
  session,
  pendingUserMessage,
  onRevise,
  attachmentPreviews,
}: {
  session: ClientSessionState;
  pendingUserMessage?: PendingUserMessage | null;
  onRevise?: (userItemId: string, text: string) => Promise<void>;
  attachmentPreviews?: AttachmentPreviews;
}) {
  const presentation = presentTimeline(session.timeline);
  const sources = messageRevisionSources(session.timeline);
  const turnsByItem = new Map(session.timeline.map((item) => [item.id, item.turnId]));
  const turnsWithErrors = new Set(session.timeline.filter((item) => item.kind === "error" && item.turnId).map((item) => item.turnId));
  const rows = presentation.map((row) => {
    if (row.kind === "message") {
      const source = sources.get(row.message.id);
      const turnId = turnsByItem.get(row.message.id);
      const summaries = attachmentSummaries(source?.raw);
      return <div key={row.key} data-timeline-kind={row.message.role}>
        {row.message.role === "user" ? <StoredAttachments key={`${session.sessionId}:${row.message.id}`} threadId={session.sessionId} userItemId={row.message.id} summaries={summaries}
          initialFiles={attachmentPreviews?.get(source?.clientItemId)} /> : null}
        <MessageBubble message={row.message} revisionHasAttachments={Boolean(summaries.length)}
        showFailureNotice={!turnId || !turnsWithErrors.has(turnId)}
        revisionText={source?.text} onRevise={source && onRevise ? (text) => onRevise(source.id, text) : undefined} /></div>;
    }
    if (row.kind === "tool" && row.item.tool) {
      return <div key={row.item.tool.callId} data-timeline-kind="tool"><ToolCallCard tool={row.item.tool} /></div>;
    }
    return <ActivityRow key={JSON.stringify([session.sessionId, row.key, row.item.text])} item={row.item} />;
  });
  const showPendingUser = Boolean(
    pendingUserMessage
      && pendingUserMessage.sessionId === session.sessionId
      && !session.timeline.some((item) => item.clientItemId === pendingUserMessage.id),
  );
  if (showPendingUser && pendingUserMessage) {
    rows.push(
      <div key={pendingUserMessage.id} data-timeline-kind="user"><AttachmentCards files={pendingUserMessage.attachments ?? []} /><MessageBubble
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
        <div key={interaction.id} data-timeline-kind="interaction" data-preserve-follow><InteractionCard interaction={interaction} /></div>
      ))}
    </div>
  );
}

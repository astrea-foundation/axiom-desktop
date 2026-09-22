import { ArrowUp, LoaderCircle, X } from "lucide-react";
import type { QueuedMessage } from "../messageQueue";
import { AttachmentCards } from "./Attachments";

export function MessageQueue({ messages, paused, canSend, onSendNow, onRemove }: {
  messages: QueuedMessage[]; paused: boolean; canSend: boolean;
  onSendNow: (id: string) => void; onRemove: (id: string) => void;
}) {
  if (!messages.length) return null;
  return <section aria-label="Message queue" className="mb-2 max-h-48 overflow-y-auto rounded-xl bg-[var(--wash-chip)] px-3 py-2 text-[12px]">
    <div className="mb-1 flex items-center gap-2 text-[var(--color-text-tertiary)]"><span>{paused ? "Queue paused" : "Queued messages"}</span><span>{messages.length}</span></div>
    {messages.map((message) => <div key={message.id} data-queued-message={message.id} className="flex items-start gap-2 py-1.5">
      <div className="min-w-0 flex-1">
        <p className="line-clamp-2 whitespace-pre-wrap break-words text-[var(--color-text-secondary)]">{message.text}</p>
        <AttachmentCards files={message.attachments ?? []} compact />
        <p className="mt-0.5 flex flex-wrap gap-x-3 text-[11px] text-[var(--color-text-tertiary)]"><span>{message.error ?? (message.status === "stopping" ? "Stopping…" : message.status === "sending" ? "Sending…" : paused ? "Paused" : "Sends after the current reply")}</span>{message.webEnabled ? <span>Web on</span> : null}</p>
      </div>
      {message.status === "queued"
        ? <button type="button" onClick={() => onSendNow(message.id)} disabled={!canSend} className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 hover:bg-[var(--wash-chip-hover)] disabled:opacity-40" title={message.preparing ? "Review the saved message before sending" : "Stop the current reply and send this message"}><ArrowUp size={12} />{message.preparing ? "Review draft" : "Send now"}</button>
        : <LoaderCircle size={14} className="mt-1 shrink-0 animate-spin text-[var(--color-text-tertiary)]" aria-label="Message pending" />}
      {message.status !== "sending" ? <button type="button" onClick={() => onRemove(message.id)} aria-label="Cancel queued message" title="Cancel queued message" className="shrink-0 rounded-md p-1.5 hover:bg-[var(--wash-chip-hover)]"><X size={13} /></button> : null}
    </div>)}
  </section>;
}

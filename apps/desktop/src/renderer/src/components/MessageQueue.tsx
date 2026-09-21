import { ArrowUp, LoaderCircle, X } from "lucide-react";
import type { QueuedMessage } from "../messageQueue";

export function MessageQueue({ messages, paused, canSteer, onSendNow, onRemove }: {
  messages: QueuedMessage[]; paused: boolean; canSteer: boolean;
  onSendNow: (id: string) => void; onRemove: (id: string) => void;
}) {
  if (!messages.length) return null;
  return <section aria-label="Message queue" className="mb-2 max-h-48 overflow-y-auto rounded-xl bg-[var(--wash-chip)] px-3 py-2 text-[12px]">
    <div className="mb-1 text-[var(--color-text-tertiary)]">{paused ? "Queue paused" : "Queued messages"} · {messages.length}</div>
    {messages.map((message) => <div key={message.id} data-queued-message={message.id} className="flex items-start gap-2 py-1.5">
      <div className="min-w-0 flex-1">
        <p className="line-clamp-2 whitespace-pre-wrap break-words text-[var(--color-text-secondary)]">{message.text}{message.attachments?.length ? ` · ${message.attachments.map((file) => file.name).join(", ")}` : ""}</p>
        <p className="mt-0.5 text-[11px] text-[var(--color-text-tertiary)]">{message.error ?? (message.status === "steering" ? "Waiting for current step to finish…" : message.status === "sending" ? "Sending…" : paused ? "Paused · Send when ready" : "Sends after the current reply")}{message.webEnabled ? " · Web on" : ""}</p>
      </div>
      {message.status === "queued" ? <>
        <button type="button" onClick={() => onSendNow(message.id)} disabled={!canSteer} className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 hover:bg-[var(--wash-chip-hover)] disabled:opacity-40" title={message.preparing ? "Review the saved message before sending" : "Continue with this message after the current operation finishes"}><ArrowUp size={12} />{message.preparing ? "Review draft" : "Send now"}</button>
        <button type="button" onClick={() => onRemove(message.id)} aria-label="Remove queued message" className="rounded-md p-1.5 hover:bg-[var(--wash-chip-hover)]"><X size={13} /></button>
      </> : <LoaderCircle size={14} className="mt-1 shrink-0 animate-spin text-[var(--color-text-tertiary)]" aria-label="Message pending" />}
    </div>)}
  </section>;
}

import { AlertTriangle, Check, ChevronDown, Copy, LoaderCircle, Lock, Pencil, RotateCcw } from "lucide-react";
import { useState } from "react";
import type { Message } from "../types";
import { ChatMarkdown } from "./ChatMarkdown";
import { DismissibleNotice } from "./DismissibleNotice";

function thoughtLabel(durationMs?: number): string {
  if (durationMs === undefined) return "Thought";
  const seconds = Math.max(1, Math.round(durationMs / 1000));
  return `Thought for ${seconds} ${seconds === 1 ? "second" : "seconds"}`;
}

export function MessageBubble({ message, onRevise, revisionText, revisionHasAttachments = false, showFailureNotice = true }: {
  message: Message; onRevise?: (text: string) => Promise<void>; revisionText?: string; revisionHasAttachments?: boolean;
  showFailureNotice?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const [revisionMode, setRevisionMode] = useState<"edit" | "regenerate" | null>(null);
  const [revisionDraft, setRevisionDraft] = useState("");
  const [revising, setRevising] = useState(false);
  const revisionControls = revisionMode ? <div data-preserve-follow className="my-2 rounded-lg border border-[var(--color-border)] p-3 text-[12px]">
    {revisionMode === "edit" ? <textarea aria-label="Edited message" value={revisionDraft} onChange={(event) => setRevisionDraft(event.target.value)}
      className="mb-2 min-h-24 w-full rounded bg-[var(--color-bg-surface)] p-2 text-[14px]" /> : null}
    <p>This replaces the reply and all later messages. Files already changed stay as they are.</p>
    <div className="mt-2 flex gap-3"><button type="button" disabled={!onRevise || revising || (!revisionHasAttachments && !(revisionMode === "edit" ? revisionDraft : revisionText)?.trim())}
      onClick={() => { setRevising(true); void onRevise?.(revisionMode === "edit" ? revisionDraft : revisionText!)
        .then(() => setRevisionMode(null)).catch(() => {}).finally(() => setRevising(false)); }}
      className="rounded bg-[var(--color-cherry)] px-3 py-1.5 text-on-accent disabled:opacity-40">{revisionMode === "edit" ? "Save and send" : "Regenerate from here"}</button>
    <button type="button" disabled={revising} onClick={() => setRevisionMode(null)}>Cancel</button></div>
  </div> : null;
  const [reasoningOpen, setReasoningOpen] = useState(false);
  const isStreaming = message.status === "streaming" || message.status === "pending";
  // Stopping a reply is not a presentation error: keep its text and allow
  // copying, without changing its stored status or claiming a final receipt.
  const terminalFailure = message.status === "failed";
  const reasoning = message.reasoningContent?.trim();

  const copyMessage = async () => {
    if (!message.content || isStreaming || terminalFailure) return;
    await navigator.clipboard.writeText(message.content);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  };

  if (message.role === "user") {
    return (
      <div className={`group flex min-w-0 justify-end${message.status === "pending" ? "" : " animate-fade-in-up"}`}>
        <div className="flex min-w-0 max-w-[min(80%,48rem)] flex-col items-end gap-1">
          {message.content ? <div className="shadow-glass-tile min-w-0 max-w-full overflow-hidden rounded-[19px] rounded-br-[8px] bg-[var(--surface-bubble-user)] px-4 py-2.5 text-[15px] leading-6 text-[var(--color-text-primary)]">
            <ChatMarkdown text={message.content} />
          </div> : null}
          {message.status === "pending" ? (
            <div role="status" aria-label="Sending message" className="flex items-center gap-1.5 px-1 py-1 text-[11px] text-[var(--color-text-tertiary)]">
              <LoaderCircle size={12} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
              Sending…
            </div>
          ) : <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
            <button
              type="button"
              className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors disabled:cursor-not-allowed disabled:opacity-35"
              aria-label="Edit message"
              disabled={!onRevise || revising}
              onClick={() => { setRevisionDraft(message.content); setRevisionMode("edit"); }}
              title="Edit and resend from this message"
            >
              <Pencil size={13} />
            </button>
          </div>}
          {revisionControls}
        </div>
      </div>
    );
  }

  return (
    <div className="group flex min-w-0 w-full max-w-full flex-col gap-1 animate-fade-in-up">
      {reasoning ? (
        <div className="w-full min-w-0 self-start text-[13px] leading-6 text-[var(--color-text-secondary)]">
          <button
            type="button"
            onClick={() => setReasoningOpen((open) => !open)}
            className="inline-flex items-center gap-1.5 rounded-md px-1.5 py-1 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
            aria-expanded={reasoningOpen}
          >
            <ChevronDown
              size={14}
              className={`transition-transform ${reasoningOpen ? "rotate-0" : "-rotate-90"}`}
            />
            <span>{thoughtLabel(message.reasoningDurationMs)}</span>
          </button>
          {reasoningOpen ? (
            <div className="mt-1.5 rounded-[10px] bg-[var(--wash-row)] px-3.5 py-2.5 text-[var(--color-text-secondary)]">
              <ChatMarkdown text={reasoning} />
            </div>
          ) : null}
        </div>
      ) : null}
      {message.content || isStreaming || terminalFailure ? (
      <div className="w-full min-w-0 max-w-full self-start px-1.5 py-1 text-[15px] leading-7 text-[var(--color-text-primary)]">
        <div className="min-w-0 max-w-full">
          {message.content ? <ChatMarkdown text={message.content} /> : null}
          {isStreaming && !message.content && !message.continues ? (
            <div className="flex items-center gap-2 py-1 text-[13px] text-[var(--color-text-tertiary)]">
              <span className="flex items-center gap-1">
                <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-text-tertiary)] dot-bounce" />
                <span
                  className="h-1.5 w-1.5 rounded-full bg-[var(--color-text-tertiary)] dot-bounce"
                  style={{ animationDelay: "0.16s" }}
                />
                <span
                  className="h-1.5 w-1.5 rounded-full bg-[var(--color-text-tertiary)] dot-bounce"
                  style={{ animationDelay: "0.32s" }}
                />
              </span>
              <span>Generating</span>
            </div>
          ) : null}
          {message.finishReason === "length" && message.terminalVerified ? <div className="mt-2 text-[12px] text-[var(--color-text-tertiary)]">The model reached its output limit. You can ask it to continue.</div> : null}
          {terminalFailure && showFailureNotice ? (
            <DismissibleNotice key={message.id} className="mt-3 flex items-center gap-2 rounded-[8px] border border-[var(--color-danger-border)] bg-[var(--color-danger-soft)] px-3 py-2 text-[12px] leading-5 text-[var(--color-danger-strong)]">
              <AlertTriangle size={13} className="shrink-0 text-[var(--color-danger)]" />
              <span className="min-w-0">This reply couldn't be verified. Try again.</span>
            </DismissibleNotice>
          ) : null}
        </div>
      </div>
      ) : null}
      {!message.continues ? <div className="flex max-w-full items-center gap-1.5 self-start opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
          <div className="flex items-center gap-0.5">
            <button
              type="button"
              onClick={() => void copyMessage()}
              disabled={!message.content || isStreaming || Boolean(terminalFailure)}
              className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)] disabled:cursor-not-allowed disabled:opacity-35"
              aria-label={copied ? "Copied" : "Copy"}
            >
              {copied ? <Check size={13} /> : <Copy size={13} />}
            </button>
            <button
              type="button"
              disabled={!onRevise || isStreaming || revising}
              onClick={() => setRevisionMode("regenerate")}
              title="Regenerate from this message"
              className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)] disabled:opacity-45"
              aria-label="Regenerate"
            >
              <RotateCcw size={13} />
            </button>
          </div>
          {message.status === "complete" && message.content && message.terminalVerified ? (
            <span className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-[var(--color-text-tertiary)]">
              <Lock size={11} className="text-[var(--color-cherry)]" />
              E2EE
            </span>
          ) : null}
      </div> : null}
      {revisionControls}
    </div>
  );
}

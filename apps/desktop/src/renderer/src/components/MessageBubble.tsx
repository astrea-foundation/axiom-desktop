import { AlertTriangle, Check, ChevronDown, Copy, Lock, Pencil, RotateCcw } from "lucide-react";
import { useState } from "react";
import type { Message } from "../types";
import { ChatMarkdown } from "./ChatMarkdown";

function thoughtLabel(durationMs?: number): string {
  if (durationMs === undefined) return "Thought";
  const seconds = Math.max(1, Math.round(durationMs / 1000));
  return `Thought for ${seconds} ${seconds === 1 ? "second" : "seconds"}`;
}

function terminalFailureLabel(status: Message["status"]): string | null {
  if (status === "failed") return "Response failed verification or execution";
  if (status === "interrupted") return "Response interrupted before verified completion";
  if (status === "cancelled") return "Response cancelled before verified completion";
  return null;
}

export function MessageBubble({ message }: { message: Message }) {
  const [copied, setCopied] = useState(false);
  const [reasoningOpen, setReasoningOpen] = useState(false);
  const isStreaming = message.status === "streaming" || message.status === "pending";
  const terminalFailure = terminalFailureLabel(message.status);
  const reasoning = message.reasoningContent?.trim();

  const copyMessage = async () => {
    if (!message.content || isStreaming || terminalFailure) return;
    await navigator.clipboard.writeText(message.content);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  };

  if (message.role === "user") {
    return (
      <div className="group flex min-w-0 justify-end animate-fade-in-up">
        <div className="flex min-w-0 max-w-[min(80%,48rem)] flex-col items-end gap-1">
          <div className="shadow-glass-tile min-w-0 max-w-full overflow-hidden rounded-[19px] rounded-br-[8px] bg-[rgba(255,255,255,0.82)] px-4 py-2.5 text-[15px] leading-6 text-[var(--color-text-primary)]">
            <ChatMarkdown text={message.content} />
          </div>
          <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
            <button
              type="button"
              className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
              aria-label="Edit message"
            >
              <Pencil size={13} />
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="group flex min-w-0 w-full max-w-full flex-col gap-1 animate-fade-in-up">
      <div className="min-w-0 max-w-full self-start rounded-[19px] rounded-bl-[8px] bg-[var(--color-cherry-glow)] px-4 py-2.5 text-[15px] leading-7 text-[var(--color-text-primary)]">
        <div className="min-w-0 max-w-full">
          {reasoning ? (
            <div className="mb-3 text-[13px] leading-6 text-[var(--color-text-secondary)]">
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
                <div className="mt-2 border-l border-[var(--color-border)] pl-3 text-[var(--color-text-secondary)]">
                  <ChatMarkdown text={reasoning} />
                </div>
              ) : null}
            </div>
          ) : null}
          {message.content ? <ChatMarkdown text={message.content} /> : null}
          {isStreaming && !message.content ? (
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
          {terminalFailure ? (
            <div className="mt-3 flex items-center gap-2 rounded-lg border border-[var(--color-danger-border)] bg-[var(--color-danger-soft)] px-3 py-2 text-[12px] leading-5 text-[var(--color-danger-strong)]">
              <AlertTriangle size={13} className="shrink-0 text-[var(--color-danger)]" />
              <span>{terminalFailure}. Partial output is unverified and cannot be copied.</span>
            </div>
          ) : null}
        </div>
      </div>
      <div className="flex max-w-full items-center gap-1.5 self-start opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
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
              disabled={isStreaming}
              className="rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)] disabled:opacity-45"
              aria-label="Regenerate"
            >
              <RotateCcw size={13} />
            </button>
          </div>
          {!isStreaming && !terminalFailure && message.content && message.terminalVerified ? (
            <span className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-[var(--color-text-tertiary)]">
              <Lock size={11} className="text-[var(--color-cherry)]" />
              E2EE
            </span>
          ) : null}
      </div>
    </div>
  );
}

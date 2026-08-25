import { ArrowDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ProviderModel, ReasoningEffort, Thread } from "../types";
import { ChatComposer } from "./ChatComposer";
import { MessageBubble } from "./MessageBubble";

interface ChatViewProps {
  thread: Thread;
  draft: string;
  onDraftChange: (value: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  isStreaming: boolean;
  models: ProviderModel[];
  selectedModelId: string;
  onModelChange: (modelId: string) => void;
  reasoningEffort?: ReasoningEffort;
  onReasoningEffortChange: (effort: ReasoningEffort | undefined) => void;
}

export function ChatView({
  thread,
  draft,
  onDraftChange,
  onSubmit,
  onStop,
  isStreaming,
  models,
  selectedModelId,
  onModelChange,
  reasoningEffort,
  onReasoningEffortChange,
}: ChatViewProps) {
  const bottomRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followRef = useRef(true);
  const [showJump, setShowJump] = useState(false);

  useEffect(() => {
    if (followRef.current) {
      bottomRef.current?.scrollIntoView({ block: "end" });
    }
  }, [thread.messages, isStreaming]);

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      <div
        ref={scrollRef}
        className="min-h-0 flex-1 overflow-y-auto px-6"
        onScroll={(event) => {
          const el = event.currentTarget;
          const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
          followRef.current = atBottom;
          setShowJump(!atBottom);
        }}
      >
        <div className="mx-auto flex w-full max-w-[760px] flex-col gap-5 pb-5 pt-12">
          {thread.messages.map((message) => (
            <MessageBubble key={message.id} message={message} />
          ))}
          <div ref={bottomRef} />
        </div>
      </div>

      {showJump ? (
        <button
          type="button"
          onClick={() => {
            followRef.current = true;
            bottomRef.current?.scrollIntoView({ block: "end", behavior: "smooth" });
            setShowJump(false);
          }}
          className="absolute bottom-[118px] left-1/2 z-10 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] shadow-lg transition-colors hover:text-[var(--color-text-primary)]"
        >
          <ArrowDown size={13} />
          Jump to latest
        </button>
      ) : null}

      <div className="shrink-0 px-6 pb-3 pt-1">
        <div className="mx-auto w-full max-w-[760px]">
          <ChatComposer
            value={draft}
            onChange={onDraftChange}
            onSubmit={onSubmit}
            onStop={onStop}
            isStreaming={isStreaming}
            models={models}
            selectedModelId={selectedModelId}
            onModelChange={onModelChange}
            reasoningEffort={reasoningEffort}
            onReasoningEffortChange={onReasoningEffortChange}
          />
        </div>
      </div>
    </div>
  );
}

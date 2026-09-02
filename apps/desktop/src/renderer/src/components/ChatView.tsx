import { ArrowDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { PendingUserMessage, ProviderModel, ReasoningEffort } from "../types";
import { ChatComposer } from "./ChatComposer";
import { TimelineView } from "./TimelineView";

interface ChatViewProps {
  session: ClientSessionState;
  draft: string;
  onDraftChange: (value: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  isStreaming: boolean;
  models: ProviderModel[];
  selectedModelId: string;
  modelStatusLabel: string;
  onModelChange: (modelId: string) => void;
  requiresSignIn: boolean;
  onSignIn: () => void;
  reasoningEffort: ReasoningEffort;
  onReasoningEffortChange: (effort: ReasoningEffort) => void;
  pendingUserMessage?: PendingUserMessage | null;
  onVerifySecurity: () => void;
}

export function ChatView({
  session,
  draft,
  onDraftChange,
  onSubmit,
  onStop,
  isStreaming,
  models,
  selectedModelId,
  modelStatusLabel,
  onModelChange,
  requiresSignIn,
  onSignIn,
  reasoningEffort,
  onReasoningEffortChange,
  pendingUserMessage,
  onVerifySecurity,
}: ChatViewProps) {
  const bottomRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followRef = useRef(true);
  const [showJump, setShowJump] = useState(false);

  useEffect(() => {
    if (followRef.current) {
      bottomRef.current?.scrollIntoView({ block: "end" });
    }
  }, [session.timeline, isStreaming, pendingUserMessage?.id]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
      <div
        ref={scrollRef}
        className="title-fade h-full overflow-y-auto px-6"
        onScroll={(event) => {
          const el = event.currentTarget;
          const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
          followRef.current = atBottom;
          setShowJump(!atBottom);
        }}
      >
        <div className="mx-auto flex w-full max-w-[760px] flex-col gap-7 pb-8 pt-[88px]">
          <TimelineView session={session} pendingUserMessage={pendingUserMessage} />
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
          aria-label="Scroll to latest"
          className="shadow-glass-pop animate-fade-in absolute bottom-3 left-1/2 z-10 flex h-9 w-9 -translate-x-1/2 items-center justify-center rounded-full bg-[var(--surface-float)] text-[var(--color-text-secondary)] transition-[transform,color] hover:text-[var(--color-text-primary)] active:scale-90"
        >
          <ArrowDown size={16} />
        </button>
      ) : null}
      </div>

      <div className="shrink-0 px-6 pb-5 pt-2">
        <div className="mx-auto w-full max-w-[760px]">
          <ChatComposer
            value={draft}
            onChange={onDraftChange}
            onSubmit={onSubmit}
            onStop={onStop}
            isStreaming={isStreaming}
            models={models}
            selectedModelId={selectedModelId}
            modelStatusLabel={modelStatusLabel}
            onModelChange={onModelChange}
            requiresSignIn={requiresSignIn}
            onSignIn={onSignIn}
            reasoningEffort={reasoningEffort}
            onReasoningEffortChange={onReasoningEffortChange}
            security={session.security}
            onVerifySecurity={onVerifySecurity}
          />
        </div>
      </div>
    </div>
  );
}

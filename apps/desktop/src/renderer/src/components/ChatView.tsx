import { ArrowDown } from "lucide-react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { PendingUserMessage } from "../types";
import { TimelineView } from "./TimelineView";
import { useChatAutoScroll } from "../useChatAutoScroll";

interface ChatViewProps {
  session: ClientSessionState;
  onRevise?: (userItemId: string, text: string) => Promise<void>;
  pendingUserMessage?: PendingUserMessage | null;
}

export function ChatView({ session, onRevise, pendingUserMessage }: ChatViewProps) {
  const autoScroll = useChatAutoScroll(session.sessionId);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
      <div
        ref={autoScroll.viewportRef}
        data-chat-scroll
        role="region"
        aria-label="Chat messages"
        tabIndex={0}
        className="title-fade h-full overflow-y-auto px-6"
        style={{ overflowAnchor: "none" }}
        {...autoScroll.handlers}
      >
        <div ref={autoScroll.contentRef} className="mx-auto flex w-full max-w-[760px] flex-col pb-8 pt-[88px]">
          <div key={session.sessionId} className="animate-fade-in flex flex-col">
            <TimelineView session={session} pendingUserMessage={pendingUserMessage} onRevise={onRevise} />
          </div>
        </div>
      </div>

      {autoScroll.showJump ? (
        <button
          type="button"
          onClick={autoScroll.resume}
          aria-label="Scroll to latest"
          className="shadow-glass-pop animate-fade-in absolute bottom-3 left-1/2 z-10 flex h-9 w-9 -translate-x-1/2 items-center justify-center rounded-full bg-[var(--surface-float)] text-[var(--color-text-secondary)] transition-[transform,color] hover:text-[var(--color-text-primary)] active:scale-90"
        >
          <ArrowDown size={16} />
        </button>
      ) : null}
      </div>


    </div>
  );
}

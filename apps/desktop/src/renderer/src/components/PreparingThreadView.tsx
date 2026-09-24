import { MessageBubble } from "./MessageBubble";
import { useChatAutoScroll } from "../useChatAutoScroll";
import { AttachmentCards } from "./Attachments";
import type { PromptAttachment } from "@axiom/axiom-acp-client";

/** Immediate local feedback while the native session and permissions are prepared. */
export function PreparingThreadView({ id, text, attachments = [] }: { id: string; text: string; attachments?: PromptAttachment[] }) {
  const autoScroll = useChatAutoScroll(id);
  return (
    <div className="flex h-full min-h-0 flex-col" aria-busy="true">
      <div ref={autoScroll.viewportRef} data-chat-scroll role="region" aria-label="Chat messages" tabIndex={0}
        className="title-fade min-h-0 flex-1 overflow-y-auto px-6" style={{ overflowAnchor: "none" }} {...autoScroll.handlers}>
        <div ref={autoScroll.contentRef} className="mx-auto flex w-full max-w-[760px] flex-col pb-8 pt-[88px]">
          <div className="chat-timeline">
            <div data-timeline-kind="user">
              <AttachmentCards files={attachments} />
              <MessageBubble message={{ id, role: "user", status: "pending", content: text }} />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

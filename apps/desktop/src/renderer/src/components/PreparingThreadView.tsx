import { MessageBubble } from "./MessageBubble";
import { useChatAutoScroll } from "../useChatAutoScroll";

/** Immediate local feedback while the native session and permissions are prepared. */
export function PreparingThreadView({ id, text }: { id: string; text: string }) {
  const autoScroll = useChatAutoScroll(id);
  return (
    <div className="flex h-full min-h-0 flex-col" aria-busy="true">
      <div ref={autoScroll.viewportRef} data-chat-scroll role="region" aria-label="Chat messages" tabIndex={0}
        className="title-fade min-h-0 flex-1 overflow-y-auto px-6" style={{ overflowAnchor: "none" }} {...autoScroll.handlers}>
        <div ref={autoScroll.contentRef} className="mx-auto flex w-full max-w-[760px] flex-col pb-8 pt-[88px]">
          <div className="chat-timeline">
            <div data-timeline-kind="user">
              <MessageBubble message={{ id, role: "user", status: "pending", content: text }} />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

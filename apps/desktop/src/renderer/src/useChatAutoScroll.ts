import { useCallback, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, PointerEvent, TouchEvent, WheelEvent } from "react";

/** Follow layout growth; only deliberate transcript interaction can pause it. */
export function useChatAutoScroll(threadId: string) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const frame = useRef<number | null>(null);
  const previousTop = useRef(0);
  const scrollbarGesture = useRef(false);
  const scrollingDown = useRef(false);
  const touchY = useRef<number | null>(null);
  const [showJump, setShowJump] = useState(false);

  const atBottom = () => {
    const el = viewportRef.current;
    return !el || el.scrollHeight - el.clientHeight - el.scrollTop <= 2;
  };
  const scrollToBottom = useCallback(() => {
    const el = viewportRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    previousTop.current = el.scrollTop;
  }, []);
  const scheduleFollow = useCallback(() => {
    if (!following.current || frame.current !== null) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      if (following.current) scrollToBottom();
    });
  }, [scrollToBottom]);
  const pause = () => {
    following.current = false;
    scrollingDown.current = false;
    setShowJump(true);
  };
  const resume = () => {
    following.current = true;
    scrollingDown.current = false;
    setShowJump(false);
    scrollToBottom();
  };
  const scrollIntent = (up: boolean) => {
    if (up) pause();
    else {
      scrollingDown.current = true;
      if (atBottom()) resume();
    }
  };

  useLayoutEffect(() => {
    following.current = true;
    scrollbarGesture.current = false;
    scrollingDown.current = false;
    touchY.current = null;
    setShowJump(false);
    scrollToBottom();
    // Includes streamed Markdown, expanded content, fonts and viewport resizing;
    // none of these layout changes constitutes an opt-out from following.
    const observer = new ResizeObserver(scheduleFollow);
    if (contentRef.current) observer.observe(contentRef.current);
    if (viewportRef.current) observer.observe(viewportRef.current);
    const endPointer = () => { scrollbarGesture.current = false; touchY.current = null; };
    window.addEventListener("pointerup", endPointer);
    window.addEventListener("pointercancel", endPointer);
    window.addEventListener("blur", endPointer);
    return () => {
      observer.disconnect();
      window.removeEventListener("pointerup", endPointer);
      window.removeEventListener("pointercancel", endPointer);
      window.removeEventListener("blur", endPointer);
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
    };
  }, [threadId, scheduleFollow, scrollToBottom]);

  const onScroll = () => {
    const el = viewportRef.current;
    if (!el) return;
    const movedUp = el.scrollTop < previousTop.current;
    if (scrollbarGesture.current && movedUp) pause();
    if (!following.current && atBottom()
        && (scrollingDown.current || (scrollbarGesture.current && !movedUp))) resume();
    previousTop.current = el.scrollTop;
    // Scroll events also come from our own writes and browser layout/anchoring.
    // Never infer a pause just because new content created a gap at the bottom.
    scheduleFollow();
  };
  const onWheelCapture = (event: WheelEvent<HTMLDivElement>) => {
    if (!event.ctrlKey && event.deltaY !== 0) scrollIntent(event.deltaY < 0);
  };
  const onPointerDownCapture = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || event.pointerType === "touch") return;
    const target = event.target as HTMLElement;
    scrollbarGesture.current = target === viewportRef.current;
    if (target.closest("[data-timeline-kind]") && !target.closest("[data-preserve-follow]")) pause();
  };
  const onClickCapture = (event: MouseEvent<HTMLDivElement>) => {
    // Also handles touch taps and keyboard activation of transcript controls.
    const target = event.target as HTMLElement;
    if (target.closest("[data-timeline-kind]") && !target.closest("[data-preserve-follow]")) pause();
  };
  const onTouchStartCapture = (event: TouchEvent<HTMLDivElement>) => {
    touchY.current = event.touches.length === 1 ? event.touches[0]!.clientY : null;
  };
  const onTouchMoveCapture = (event: TouchEvent<HTMLDivElement>) => {
    if (touchY.current === null || event.touches.length !== 1) return;
    const next = event.touches[0]!.clientY;
    if (Math.abs(next - touchY.current) >= 2) {
      scrollIntent(next > touchY.current);
      touchY.current = next;
    }
  };
  const onKeyDownCapture = (event: KeyboardEvent<HTMLDivElement>) => {
    if ((event.target as HTMLElement).closest("input, textarea, select, [contenteditable=true]")) return;
    if (["ArrowUp", "PageUp", "Home"].includes(event.key) || (event.key === " " && event.shiftKey)) scrollIntent(true);
    if (["ArrowDown", "PageDown", "End"].includes(event.key) || (event.key === " " && !event.shiftKey)) scrollIntent(false);
  };

  return {
    viewportRef, contentRef, showJump, resume,
    handlers: { onScroll, onWheelCapture, onPointerDownCapture, onClickCapture, onTouchStartCapture, onTouchMoveCapture, onKeyDownCapture },
  };
}

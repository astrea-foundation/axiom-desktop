import { useCallback, useLayoutEffect, useRef, useState, type AnimationEvent } from "react";

// Matches the exit animations in index.css. The timer covers interrupted events.
const EXIT_MS = 160;
const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
const isExit = (event: AnimationEvent) => event.target === event.currentTarget && event.animationName.endsWith("-out");

/** Keep a controlled overlay's last value mounted while it exits. Reopening cancels exit. */
export function useOverlayPresence<T>(value: T | null) {
  const [retained, setRetained] = useState(value);
  const closing = value === null && retained !== null;
  useLayoutEffect(() => {
    if (value !== null) { setRetained(value); return; }
    if (reducedMotion()) { setRetained(null); return; }
    const timer = setTimeout(() => setRetained(null), EXIT_MS + 50);
    return () => clearTimeout(timer);
  }, [value]);
  return {
    value: value ?? retained,
    props: {
      "data-closing": closing || undefined,
      inert: closing,
      onAnimationEnd: (event: AnimationEvent) => { if (closing && isExit(event)) setRetained(null); },
    },
  };
}

/** Finish a modal action after panel/backdrop exit, before navigation restores focus. */
export function useAnimatedClose() {
  const [closing, setClosing] = useState(false);
  const action = useRef<(() => void) | null>(null);
  const complete = useCallback(() => {
    const next = action.current;
    action.current = null;
    next?.();
  }, []);
  const close = useCallback((next: () => void) => {
    if (action.current) return;
    if (reducedMotion()) { next(); return; }
    action.current = next;
    setClosing(true);
  }, []);
  useLayoutEffect(() => {
    if (!closing) return;
    const timer = setTimeout(complete, EXIT_MS + 50);
    return () => clearTimeout(timer);
  }, [closing, complete]);
  useLayoutEffect(() => () => { action.current = null; }, []);
  return {
    close,
    props: {
      "data-closing": closing || undefined,
      inert: closing,
      onAnimationEnd: (event: AnimationEvent) => { if (closing && isExit(event)) complete(); },
    },
  };
}

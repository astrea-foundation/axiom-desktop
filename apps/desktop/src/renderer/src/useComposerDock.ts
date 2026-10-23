import { useLayoutEffect, useRef } from "react";

/** Move the same composer from its welcome position to the conversation footer. */
export function useComposerDock(docked: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  const origin = useRef<DOMRect | null>(null);
  const animation = useRef<Animation | null>(null);
  const capture = () => { origin.current = ref.current?.getBoundingClientRect() ?? null; };
  useLayoutEffect(() => {
    const element = ref.current;
    const from = origin.current;
    origin.current = null;
    animation.current?.cancel();
    if (!docked || !element || !from || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const to = element.getBoundingClientRect();
    animation.current = element.animate([
      { transform: `translate(${from.left - to.left}px, ${from.top - to.top}px)` },
      { transform: "none" },
    ], { duration: 420, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
    return () => animation.current?.cancel();
  }, [docked]);
  return { ref, capture };
}

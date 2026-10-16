import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";

const FADE_WIDTH = 24;
const SCROLL_SPEED = 32;

/** Fade only actual overflow, and reveal its tail at a readable, fixed speed. */
export function OverflowTitle({ text, className = "" }: { text: string; className?: string }) {
  const viewport = useRef<HTMLSpanElement>(null);
  const content = useRef<HTMLSpanElement>(null);
  const [distance, setDistance] = useState(0);
  const [reducedMotion, setReducedMotion] = useState(false);

  useLayoutEffect(() => {
    const outer = viewport.current;
    const inner = content.current;
    if (!outer || !inner) return;
    const measure = () => {
      const overflow = inner.getBoundingClientRect().width - outer.clientWidth;
      // The extra space places the final character before the fading edge.
      setDistance(outer.clientWidth > 0 && overflow > 1 ? Math.ceil(overflow) + FADE_WIDTH : 0);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(outer);
    observer.observe(inner); // Includes font loading and title changes.
    return () => observer.disconnect();
  }, [text]);

  useEffect(() => {
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReducedMotion(preference.matches);
    update();
    preference.addEventListener("change", update);
    return () => preference.removeEventListener("change", update);
  }, []);

  return (
    <span ref={viewport} className={`overflow-title ${className}`} data-overflow={distance > 0 ? "true" : undefined}
      title={distance > 0 && reducedMotion ? text : undefined}
      style={{
        "--title-scroll-distance": `${-distance}px`,
        "--title-scroll-duration": `${Math.max(3, distance / SCROLL_SPEED + 1.5)}s`,
      } as CSSProperties}>
      <span key={text} className="overflow-title-track"><span ref={content} className="overflow-title-text">{text}</span></span>
    </span>
  );
}

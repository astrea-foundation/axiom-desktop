/** @jsxRuntime automatic */
import { useChatLocale } from "./ChatLocale";
import { useEffect, useRef, useState } from "react";

const GREETINGS = [
  "greeting.thinkFreely",
  "greeting.offTheRecord",
  "greeting.nobodyListening",
  "greeting.quietPlace",
  "greeting.speakFreely",
  "greeting.yourWords",
] as const;

export function GreetingHeadline() {
  const ref = useRef<HTMLHeadingElement>(null);
  const { t } = useChatLocale();
  const [greeting] = useState(() => GREETINGS[Math.floor(Math.random() * GREETINGS.length)] ?? GREETINGS[0]);
  const line = t(greeting);
  const words = line.split(" ");

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let cancelled = false;
    // Rise only after PP Cirka is ready, so the unmask never plays in a fallback
    // face. fonts.load() forces the face to start loading; ready alone can
    // resolve before a lazily-triggered face finishes.
    void Promise.all([document.fonts.load('400 1em "PP Cirka"'), document.fonts.ready])
      .catch(() => undefined)
      .then(() => {
        // Double rAF: the first frame paints the masked state, the second rises.
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            if (!cancelled) el.classList.add("unmask-in");
          });
        });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <h1
      ref={ref}
      className="mb-9 text-center text-[42px] font-normal leading-[0.98] tracking-[-0.045em] text-[var(--color-text-primary)] sm:text-[50px] md:text-[56px]"
      style={{ fontFamily: "var(--font-display)" }}
      aria-label={line}
    >
      {words.map((word, i) => (
        <span key={i} className="unmask unmask-word align-bottom" aria-hidden="true">
          <span className="unmask-inner" style={{ "--ld": `${0.08 + i * 0.07}s` } as React.CSSProperties}>
            {word}
            {i < words.length - 1 ? "\u00a0" : ""}
          </span>
        </span>
      ))}
    </h1>
  );
}

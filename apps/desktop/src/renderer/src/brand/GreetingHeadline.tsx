import { animate, stagger } from "animejs";
import { useEffect, useRef, useState } from "react";

const GREETINGS = [
  "Think freely.",
  "Off the record.",
  "Nobody's listening.",
  "A quiet place to think.",
  "Speak freely.",
  "Your words stay yours.",
];

function pickGreeting(): string {
  return GREETINGS[Math.floor(Math.random() * GREETINGS.length)] ?? "Think freely.";
}

export function GreetingHeadline() {
  const ref = useRef<HTMLHeadingElement>(null);
  const didAnimate = useRef(false);
  const [line] = useState(pickGreeting);
  const words = line.split(" ");

  useEffect(() => {
    if (didAnimate.current) return;
    didAnimate.current = true;

    const el = ref.current;
    if (!el) return;
    const wordEls = el.querySelectorAll<HTMLElement>("[data-word]");
    if (!wordEls.length) return;

    const prefersReduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    if (prefersReduced) {
      wordEls.forEach((w) => {
        w.style.opacity = "1";
        w.style.transform = "none";
        w.style.filter = "none";
      });
      return;
    }

    animate(wordEls, {
      opacity: [0, 1],
      translateY: [18, 0],
      filter: ["blur(7px)", "blur(0px)"],
      duration: 900,
      delay: stagger(75, { start: 90 }),
      ease: "outExpo",
    });
  }, []);

  return (
    <h1
      ref={ref}
      className="mb-8 text-center text-[32px] leading-tight tracking-tight text-[var(--color-text-primary)] sm:text-[40px] md:text-[44px]"
      aria-label={line}
    >
      {words.map((word, i) => (
        <span
          key={i}
          data-word
          aria-hidden="true"
          className="inline-block opacity-0"
          style={{ willChange: "transform, opacity, filter" }}
        >
          {word}
          {i < words.length - 1 ? "\u00a0" : ""}
        </span>
      ))}
    </h1>
  );
}

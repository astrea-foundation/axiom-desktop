import { useEffect, useRef } from "react";
import Sakura from "../lib/sakura";
import "../lib/sakura.css";

export function CherryBlossomBackground() {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;

    const sakura = new Sakura(el, {
      fallSpeed: 3,
      delay: 2000,
      minSize: 9,
      maxSize: 15,
      originBiasRight: 0.8,
      colors: [
        {
          gradientColorStart: "rgba(255, 226, 235, 0.9)",
          gradientColorEnd: "rgba(255, 184, 200, 0.85)",
          gradientColorDegree: 120,
        },
        {
          gradientColorStart: "rgba(255, 184, 200, 0.9)",
          gradientColorEnd: "rgba(255, 123, 148, 0.85)",
          gradientColorDegree: 120,
        },
        {
          gradientColorStart: "rgba(255, 107, 133, 0.85)",
          gradientColorEnd: "rgba(230, 61, 91, 0.8)",
          gradientColorDegree: 120,
        },
      ],
    });

    return () => sakura.stop();
  }, []);

  return <div ref={ref} className="pointer-events-none absolute inset-0 overflow-hidden" />;
}

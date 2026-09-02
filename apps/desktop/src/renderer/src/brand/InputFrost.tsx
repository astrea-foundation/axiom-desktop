import { useEffect, useRef, type RefObject } from "react";

interface InputFrostProps {
  targetRef: RefObject<HTMLElement | null>;
  containerRef: RefObject<HTMLElement | null>;
}

export function InputFrost({ targetRef, containerRef }: InputFrostProps) {
  const frostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const frost = frostRef.current;
    const target = targetRef.current;
    const container = containerRef.current;
    if (!frost || !target || !container) return;

    const sync = () => {
      const c = container.getBoundingClientRect();
      const t = target.getBoundingClientRect();
      frost.style.left = `${t.left - c.left}px`;
      frost.style.top = `${t.top - c.top}px`;
      frost.style.width = `${t.width}px`;
      frost.style.height = `${t.height}px`;
    };

    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(target);
    ro.observe(container);
    window.addEventListener("resize", sync);
    if (document.fonts?.ready) document.fonts.ready.then(sync).catch(() => {});

    return () => {
      ro.disconnect();
      window.removeEventListener("resize", sync);
    };
  }, [targetRef, containerRef]);

  return (
    <div
      ref={frostRef}
      aria-hidden="true"
      className="pointer-events-none absolute left-0 top-0 rounded-[26px]"
      style={{ backdropFilter: "blur(14px)", WebkitBackdropFilter: "blur(14px)" }}
    />
  );
}

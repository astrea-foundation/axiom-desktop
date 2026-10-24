import { ArrowLeft } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

/** Keep the screen mounted during exit; chat and its draft remain underneath. */
export function AccountScreen({ title, onClose, wide = false, children }: {
  title: string;
  onClose: () => void;
  wide?: boolean;
  children: ReactNode;
}) {
  const [closing, setClosing] = useState(false);
  const finish = useRef(onClose);
  finish.current = onClose;
  const close = useCallback(() => setClosing(true), []);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); close(); }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [close]);
  useEffect(() => {
    if (!closing) return;
    // A timer also completes exits if animation events are interrupted.
    const duration = matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 180;
    const timer = setTimeout(() => finish.current(), duration);
    return () => clearTimeout(timer);
  }, [closing]);

  return (
    <div role="region" aria-label={title} data-closing={closing || undefined} inert={closing}
      className="account-screen-motion app-canvas fixed inset-0 z-40 flex flex-col overflow-hidden px-4 pb-5 pt-12 outline-none sm:px-8 sm:pb-8 sm:pt-14">
      <div className="drag-region absolute inset-x-0 top-0 h-11" aria-hidden="true" />
      <div className={`mx-auto flex min-h-0 w-full flex-1 flex-col ${wide ? "max-w-[1000px]" : "max-w-[720px]"}`}>
        <header className="mb-5 flex shrink-0 items-center gap-4 sm:mb-6">
          <button type="button" onClick={close} className="ax-pill ax-pill-button no-drag"><ArrowLeft size={13} />Back</button>
          <h1 className="text-[22px] font-medium tracking-[-0.025em] text-[var(--color-text-primary)]">{title}</h1>
        </header>
        {children}
      </div>
    </div>
  );
}

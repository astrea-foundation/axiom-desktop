import { Minus, Square, X } from "lucide-react";
import { useEffect, useState } from "react";
import { hasNativeWindowControls } from "../lib/window-chrome";

export function WindowControls() {
  if (hasNativeWindowControls) return null;
  return <FramelessWindowControls />;
}

const CONTROL_CLASS =
  "flex h-7 w-7 items-center justify-center rounded-full text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-hover-strong)] hover:text-[var(--color-text-primary)]";

function FramelessWindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const desktop = window.axiomDesktop;
    if (!desktop) return;
    void desktop.isMaximized().then(setMaximized);
    return desktop.onMaximizedChange(setMaximized);
  }, []);

  const desktop = window.axiomDesktop;

  return (
    <div className="no-drag relative z-40 flex shrink-0 items-center gap-0.5">
      <button type="button" onClick={() => desktop?.minimize()} className={CONTROL_CLASS} aria-label="Minimize" title="Minimize">
        <Minus size={14} />
      </button>
      <button
        type="button"
        onClick={() => desktop?.maximize()}
        className={CONTROL_CLASS}
        aria-label={maximized ? "Restore" : "Maximize"}
        title={maximized ? "Restore" : "Maximize"}
      >
        <Square size={11} />
      </button>
      <button
        type="button"
        onClick={() => desktop?.close()}
        className={`${CONTROL_CLASS} hover:text-[var(--color-danger)]`}
        aria-label="Close"
        title="Close"
      >
        <X size={14} />
      </button>
    </div>
  );
}

import { Minus, Square, X } from "lucide-react";
import { useEffect, useState } from "react";
import { hasNativeWindowControls } from "../lib/window-chrome";

const CONTROL_CLASS =
  "no-drag pointer-events-auto flex h-7 w-7 items-center justify-center rounded-full text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-hover-strong)] hover:text-[var(--color-text-primary)]";

export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const desktop = window.axiomDesktop;
    if (hasNativeWindowControls || !desktop) return;
    void desktop.isMaximized().then(setMaximized);
    return desktop.onMaximizedChange(setMaximized);
  }, []);

  if (hasNativeWindowControls) return null;
  const desktop = window.axiomDesktop;

  return (
    <div className="window-controls no-drag pointer-events-auto fixed right-3 top-2 z-50 flex items-center gap-0.5">
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

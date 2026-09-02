import { Minus, Square, X } from "lucide-react";
import { useEffect, useState } from "react";
import { hasNativeWindowControls } from "../lib/window-chrome";

export function WindowControls() {
  if (hasNativeWindowControls) return null;
  return <FramelessWindowControls />;
}

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
    <div className="no-drag relative z-40 flex h-11 shrink-0 items-center">
      <button
        type="button"
        onClick={() => desktop?.minimize()}
        className="flex h-11 w-11 items-center justify-center text-[var(--color-text-secondary)] transition-colors hover:bg-[rgba(16,16,20,0.06)] hover:text-[var(--color-text-primary)]"
        aria-label="Minimize"
        title="Minimize"
      >
        <Minus size={14} />
      </button>
      <button
        type="button"
        onClick={() => desktop?.maximize()}
        className="flex h-11 w-11 items-center justify-center text-[var(--color-text-secondary)] transition-colors hover:bg-[rgba(16,16,20,0.06)] hover:text-[var(--color-text-primary)]"
        aria-label={maximized ? "Restore" : "Maximize"}
        title={maximized ? "Restore" : "Maximize"}
      >
        <Square size={11} />
      </button>
      <button
        type="button"
        onClick={() => desktop?.close()}
        className="flex h-11 w-11 items-center justify-center text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-danger)] hover:text-white"
        aria-label="Close"
        title="Close"
      >
        <X size={14} />
      </button>
    </div>
  );
}

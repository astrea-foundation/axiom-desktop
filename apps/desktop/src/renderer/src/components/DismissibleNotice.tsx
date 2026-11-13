import { X } from "lucide-react";
import { useState, type HTMLAttributes } from "react";

/** Key by notice identity so dismissing an old error cannot hide a new one. */
export function DismissibleNotice({ children, className = "", role = "alert", onDismiss, ...props }:
  HTMLAttributes<HTMLDivElement> & { onDismiss?: () => void }) {
  const [dismissed, setDismissed] = useState(false);
  if (dismissed) return null;
  return <div {...props} role={role} className={`flex items-start gap-2 ${className}`}>
    {children}
    <button type="button" aria-label="Dismiss notice" title="Dismiss"
      className="-my-0.5 ml-auto flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-current opacity-65 transition-opacity hover:bg-[var(--wash-hover)] hover:opacity-100"
      onClick={(event) => {
        event.currentTarget.closest<HTMLElement>("[data-chat-scroll]")?.focus({ preventScroll: true });
        setDismissed(true);
        onDismiss?.();
      }}>
      <X size={14} aria-hidden="true" />
    </button>
  </div>;
}

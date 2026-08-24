import { X } from "lucide-react";
import { useEffect } from "react";

export function SettingsSheet({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="absolute inset-0 z-50 flex items-start justify-center bg-black/55 px-6 pt-24 backdrop-blur-sm">
      <div
        role="dialog"
        aria-label="Settings"
        className="animate-panel-in w-full max-w-[440px] rounded-2xl border border-[var(--color-border)] bg-[var(--color-bg-surface)] p-5 shadow-2xl"
      >
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-[16px] font-medium">Settings</h2>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md p-1 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)]"
            aria-label="Close settings"
          >
            <X size={16} />
          </button>
        </div>
        <div className="flex flex-col gap-3 text-[13.5px] text-[var(--color-text-secondary)]">
          <label className="flex items-center justify-between gap-3 rounded-xl bg-[var(--color-bg-input)] px-3 py-3">
            <span className="text-[var(--color-text-primary)]">Appearance</span>
            <span className="text-[var(--color-text-tertiary)]">Dark</span>
          </label>
          <label className="flex items-center justify-between gap-3 rounded-xl bg-[var(--color-bg-input)] px-3 py-3">
            <span className="text-[var(--color-text-primary)]">Key</span>
            <span className="font-mono text-[12.5px] text-[var(--color-text-tertiary)]">
              axm_…4f2
            </span>
          </label>
        </div>
      </div>
    </div>
  );
}

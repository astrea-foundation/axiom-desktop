import { X } from "lucide-react";
import { useId, useLayoutEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";

export function CryptoDepositDialog({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const backdropPressed = useRef(false);
  const title = useId();
  useLayoutEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    return () => { element.close(); if (previous?.isConnected) previous.focus({ preventScroll: true }); };
  }, []);
  const close = () => { dialog.current?.close(); onClose(); };
  const outside = (event: React.PointerEvent<HTMLDialogElement> | React.MouseEvent<HTMLDialogElement>) => {
    if (event.target !== event.currentTarget) return false;
    const bounds = event.currentTarget.getBoundingClientRect();
    return event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom;
  };
  return createPortal(<dialog ref={dialog} aria-modal="true" aria-labelledby={title} style={{ background: "var(--color-bg-surface)" }}
    onCancel={(event) => { event.preventDefault(); close(); }}
    onKeyDown={(event) => { if (event.key === "Escape") event.stopPropagation(); }}
    onPointerDown={(event) => { backdropPressed.current = outside(event); }}
    onClick={(event) => { if (backdropPressed.current && outside(event)) close(); }}
    className="crypto-deposit-dialog no-drag glass-panel-solid shadow-glass-pop m-auto w-[440px] max-w-[calc(100%_-_32px)] overflow-visible rounded-[22px] border border-[var(--color-border)] p-0 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]">
    <div className="max-h-[calc(100dvh-48px)] overflow-y-auto overscroll-contain rounded-[22px] p-6 sm:p-7">
      <header className="mb-6 flex items-center justify-between gap-4">
        <h2 id={title} className="text-[22px] font-medium tracking-[-0.025em]">Deposit crypto</h2>
        <button type="button" aria-label="Close deposit" onClick={close}
          className="rounded-full p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline-2 focus-visible:outline-offset-2"><X size={18} aria-hidden="true" /></button>
      </header>
      {children}
    </div>
  </dialog>, document.body);
}

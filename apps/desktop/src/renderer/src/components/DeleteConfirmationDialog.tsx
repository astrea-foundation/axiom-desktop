import { useId, useLayoutEffect, useRef } from "react";

interface DeleteConfirmationDialogProps {
  kind: "thread" | "folder";
  title: string;
  onCancel: () => void;
  onConfirm: () => void;
}

/** A renderer-owned modal: never open an OS/GTK JavaScript confirmation. */
export function DeleteConfirmationDialog({ kind, title, onCancel, onConfirm }: DeleteConfirmationDialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  useLayoutEffect(() => {
    const dialog = ref.current;
    dialog?.showModal();
    return () => { dialog?.close(); };
  }, []);

  const finish = (action: () => void) => {
    // Remove modal inertness before navigation mounts/focuses the new composer.
    // The sidebar's Delete menu item may already be unmounted, so native focus
    // restoration alone cannot return focus to a useful control.
    ref.current?.close();
    action();
    document.querySelector<HTMLTextAreaElement>("[data-chat-input]")?.focus({ preventScroll: true });
  };

  return (
    <dialog
      ref={ref}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={descriptionId}
      onCancel={(event) => {
        event.preventDefault();
        finish(onCancel);
      }}
      onKeyDown={(event) => {
        if (event.key !== "Tab") return;
        const buttons = ref.current?.querySelectorAll<HTMLButtonElement>("button");
        const first = buttons?.[0];
        const last = buttons?.[buttons.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first?.focus();
        }
      }}
      className="no-drag glass-panel-solid shadow-glass-pop m-auto w-[460px] max-w-[calc(100%_-_40px)] rounded-[22px] border-0 p-7 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]"
    >
      <h2 id={titleId} className="text-[20px] font-medium tracking-tight">Delete {kind}?</h2>
      <p id={descriptionId} className="mt-3 break-words text-[13.5px] leading-5 text-[var(--color-text-secondary)]">
        {kind === "thread"
          ? `“${title}” will be permanently deleted. Any active response will be stopped.`
          : `“${title}” will be deleted. Its threads will remain available.`}
      </p>
      <div className="mt-6 flex justify-end gap-2">
        <button
          type="button"
          autoFocus
          onClick={() => finish(onCancel)}
          className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] hover:bg-[var(--wash-chip-hover)]"
        >Cancel</button>
        <button
          type="button"
          onClick={() => finish(onConfirm)}
          className="rounded-lg bg-[var(--color-danger-soft)] px-4 py-2 text-[13px] text-[var(--color-danger-strong)] hover:bg-[var(--wash-chip-hover)]"
        >Delete {kind}</button>
      </div>
    </dialog>
  );
}

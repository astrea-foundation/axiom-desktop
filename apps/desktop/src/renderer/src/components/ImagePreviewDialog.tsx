import { useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

export function ImagePreviewDialog({ name, src, onClose }: {
  name: string; src: string; onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const backdropPressed = useRef(false);
  useLayoutEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);

  const close = () => {
    dialog.current?.close();
    onClose();
  };
  const outside = (event: React.PointerEvent<HTMLDialogElement> | React.MouseEvent<HTMLDialogElement>) => {
    if (event.target !== event.currentTarget) return false;
    const bounds = event.currentTarget.getBoundingClientRect();
    return event.clientX < bounds.left || event.clientX > bounds.right
      || event.clientY < bounds.top || event.clientY > bounds.bottom;
  };

  return createPortal(<dialog ref={dialog} aria-label={`Image preview: ${name}`} aria-modal="true"
    onCancel={(event) => { event.preventDefault(); close(); }}
    onPointerDown={(event) => { backdropPressed.current = outside(event); }}
    onClick={(event) => { if (backdropPressed.current && outside(event)) close(); }}
    onKeyDown={(event) => {
      if (event.key === "Tab") { event.preventDefault(); closeButton.current?.focus(); }
    }}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto h-[calc(100%_-_64px)] max-h-[900px] w-[calc(100%_-_32px)] max-w-[1200px] overflow-hidden rounded-[22px] border-0 p-0 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]"
  >
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-4 px-5 py-3">
        <p className="min-w-0 flex-1 truncate text-sm" title={name}>{name}</p>
        <button ref={closeButton} type="button" autoFocus aria-label="Close image preview" onClick={close}
          className="shrink-0 rounded-full p-2 text-[var(--color-text-secondary)] hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline-2 focus-visible:outline-offset-2">
          <X size={20} aria-hidden="true" />
        </button>
      </div>
      <div className="min-h-0 flex-1 p-4 pt-0">
        <img src={src} alt={name} draggable={false} className="h-full w-full object-contain" />
      </div>
    </div>
  </dialog>, document.body);
}

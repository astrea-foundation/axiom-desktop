/** @jsxRuntime automatic */
import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

export function ImagePreviewDialog({ name, src, onClose }: {
  name: string; src: string; onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const zoomButton = useRef<HTMLButtonElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const [zoom, setZoom] = useState<{ width: number; height: number } | null>(null);
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
      if (event.key === "Tab") {
        event.preventDefault();
        const buttons = [closeButton.current, zoomButton.current].filter((button): button is HTMLButtonElement => !!button);
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        buttons[(current + (event.shiftKey ? -1 : 1) + buttons.length) % buttons.length]?.focus();
      }
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
      <div ref={viewport} className="m-4 mt-0 min-h-0 flex-1 overflow-auto overscroll-contain">
        <button ref={zoomButton} type="button" aria-label={zoom ? "Zoom out" : "Zoom in"}
          aria-pressed={!!zoom} title={zoom ? "Click to fit image" : "Click to zoom in"}
          className={zoom ? "block cursor-zoom-out" : "block h-full w-full cursor-zoom-in"}
          style={zoom ?? undefined}
          onClick={event => {
            const container = viewport.current;
            if (!container) return;
            if (zoom) { setZoom(null); container.scrollTo(0, 0); return; }
            const image = event.currentTarget.querySelector("img");
            if (!image?.naturalWidth || !image.naturalHeight) return;
            const fit = Math.min(container.clientWidth / image.naturalWidth, container.clientHeight / image.naturalHeight);
            const scale = Math.max(1, fit * 2);
            const size = { width: Math.round(image.naturalWidth * scale), height: Math.round(image.naturalHeight * scale) };
            const bounds = event.currentTarget.getBoundingClientRect();
            const x = event.detail ? Math.min(1, Math.max(0, (event.clientX - bounds.left - (bounds.width - image.naturalWidth * fit) / 2) / (image.naturalWidth * fit))) : .5;
            const y = event.detail ? Math.min(1, Math.max(0, (event.clientY - bounds.top - (bounds.height - image.naturalHeight * fit) / 2) / (image.naturalHeight * fit))) : .5;
            setZoom(size);
            requestAnimationFrame(() => container.scrollTo(x * size.width - container.clientWidth / 2, y * size.height - container.clientHeight / 2));
          }}>
          <img src={src} alt={name} draggable={false} className="h-full w-full object-contain" />
        </button>
      </div>
    </div>
  </dialog>, document.body);
}

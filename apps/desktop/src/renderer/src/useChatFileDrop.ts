import { useEffect, useRef, useState, type DragEvent } from "react";

/** Only OS file drags belong to the composer; thread moves and text keep their handlers. */
export function useChatFileDrop(enabled: boolean, scope: string, onFiles: (files: File[]) => void) {
  const depth = useRef(0);
  const [dragging, setDragging] = useState(false);
  const reset = () => { depth.current = 0; setDragging(false); };
  useEffect(() => {
    reset();
    window.addEventListener("dragend", reset);
    window.addEventListener("drop", reset);
    window.addEventListener("blur", reset);
    return () => {
      window.removeEventListener("dragend", reset);
      window.removeEventListener("drop", reset);
      window.removeEventListener("blur", reset);
    };
  }, [enabled, scope]);
  // Portalled dialogs bubble through React's chat tree but are outside the panel.
  const hasFiles = (event: DragEvent) => event.currentTarget.contains(event.target as Node)
    && event.dataTransfer.types.includes("Files");
  return {
    dragging: enabled && dragging,
    handlers: {
      onDragEnterCapture: (event: DragEvent) => {
        if (!hasFiles(event)) return;
        event.preventDefault();
        depth.current += 1;
        if (enabled) setDragging(true);
      },
      onDragLeaveCapture: (event: DragEvent) => {
        if (!hasFiles(event)) return;
        depth.current = Math.max(0, depth.current - 1);
        if (!depth.current) setDragging(false);
      },
      onDragOverCapture: (event: DragEvent) => {
        if (!hasFiles(event)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = enabled ? "copy" : "none";
      },
      onDropCapture: (event: DragEvent) => {
        if (!hasFiles(event)) return;
        event.preventDefault();
        event.stopPropagation();
        reset();
        if (enabled && event.dataTransfer.files.length) onFiles(Array.from(event.dataTransfer.files));
      },
    },
  };
}

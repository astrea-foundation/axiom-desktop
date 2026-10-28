import { Pencil, Trash2 } from "lucide-react";
import { useLayoutEffect, useRef, type KeyboardEvent, type MouseEvent } from "react";
import { createPortal } from "react-dom";

export interface SidebarMenuTarget {
  kind: "thread" | "folder";
  id: string;
  x: number;
  y: number;
  trigger: HTMLElement;
}

export function contextMenuTrigger(
  kind: SidebarMenuTarget["kind"],
  id: string,
  open: (target: SidebarMenuTarget) => void,
) {
  const show = (event: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const trigger = event.currentTarget;
    const rect = trigger.getBoundingClientRect();
    const pointer = "clientX" in event && Number.isFinite(event.clientX) && Number.isFinite(event.clientY)
      && (event.clientX !== 0 || event.clientY !== 0);
    open({ kind, id, trigger, x: pointer ? event.clientX : rect.left + 16, y: pointer ? event.clientY : rect.bottom });
  };
  return {
    "aria-haspopup": "menu" as const,
    onContextMenu: show,
    onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) show(event);
    },
  };
}

export function SidebarContextMenu({ target, onClose, onRename, onDelete }: {
  target: SidebarMenuTarget;
  onClose: () => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const dismiss = (restoreFocus: boolean) => {
    if (restoreFocus && target.trigger.isConnected) target.trigger.focus({ preventScroll: true });
    onClose();
  };

  useLayoutEffect(() => {
    const menu = ref.current!;
    const rect = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(target.x, window.innerWidth - rect.width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(target.y, window.innerHeight - rect.height - 8))}px`;
    menu.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    const outside = (event: Event) => {
      if (!menu.contains(event.target as Node)) onClose();
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("contextmenu", outside, true);
    document.addEventListener("scroll", outside, true);
    window.addEventListener("resize", onClose);
    window.addEventListener("blur", onClose);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("contextmenu", outside, true);
      document.removeEventListener("scroll", outside, true);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("blur", onClose);
    };
  }, [target, onClose]);

  return createPortal(
    <div
      ref={ref}
      role="menu"
      aria-label={`${target.kind === "thread" ? "Thread" : "Folder"} actions`}
      style={{ left: target.x, top: target.y }}
      className="no-drag glass-panel-solid shadow-glass-pop fixed z-[100] w-40 rounded-[10px] p-1"
      onContextMenu={(event) => event.preventDefault()}
      onKeyDown={(event) => {
        if (event.key === "Escape" || event.key === "Tab") {
          event.preventDefault();
          event.stopPropagation();
          dismiss(true);
        } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          const items = Array.from(ref.current!.querySelectorAll<HTMLButtonElement>("button"));
          const current = items.indexOf(document.activeElement as HTMLButtonElement);
          const index = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
            : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
          items[index]?.focus();
        }
      }}
    >
      <button type="button" role="menuitem" onClick={() => { dismiss(true); onRename(); }}
        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12.5px] text-[var(--color-text-primary)] hover:bg-[var(--wash-hover)] focus:bg-[var(--wash-hover)] focus:outline-none">
        <Pencil size={13} />Rename
      </button>
      <button type="button" role="menuitem" onClick={() => { dismiss(true); onDelete(); }}
        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12.5px] text-[var(--color-danger-strong)] hover:bg-[var(--color-danger-soft)] focus:bg-[var(--color-danger-soft)] focus:outline-none">
        <Trash2 size={13} />Delete
      </button>
    </div>, document.body,
  );
}

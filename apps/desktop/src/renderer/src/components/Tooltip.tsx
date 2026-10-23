import { useOverlayPresence } from "../useOverlayMotion";
import { useId, useState, type ReactElement, type ReactNode } from "react";
import { cloneElement } from "react";

/** Hover/focus tooltip for a single control. Instant, styled and animated, unlike a native title. */
export function Tooltip({ label, children, align = "left" }: { label: ReactNode; children: ReactElement<Record<string, unknown>>; align?: "left" | "right" }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const presence = useOverlayPresence(open ? true : null);
  const childProps = children.props as Record<string, unknown>;
  const chain = (name: string, handler: () => void) => (event: unknown) => {
    (childProps[name] as ((event: unknown) => void) | undefined)?.(event);
    handler();
  };
  return <span className="relative inline-flex shrink-0" onPointerEnter={() => setOpen(true)} onPointerLeave={() => setOpen(false)}>
    {cloneElement(children, {
      "aria-describedby": open ? id : undefined,
      onFocus: chain("onFocus", () => setOpen(true)),
      onBlur: chain("onBlur", () => setOpen(false)),
    })}
    {presence.value ? <span {...presence.props} role="tooltip" id={id}
      className={`shadow-glass-pop popover-motion pointer-events-none absolute bottom-full z-50 mb-2 w-max max-w-[240px] rounded-[9px] bg-[var(--surface-float)] px-2.5 py-1.5 text-[11.5px] leading-[16px] text-[var(--color-text-secondary)] backdrop-blur-xl ${align === "right" ? "right-0 origin-bottom-right" : "left-0 origin-bottom-left"}`}>
      {label}
    </span> : null}
  </span>;
}

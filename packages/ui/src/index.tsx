import { useEffect, useRef, type ReactNode } from "react";
import { AlertCircle, Check, Info, LoaderCircle, X } from "lucide-react";

export function Surface({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <section className={`ax-surface ${className}`.trim()}>{children}</section>;
}
export function Spinner({ label = "Loading" }: { label?: string }) {
  return <LoaderCircle className="ax-spinner" size={17} role="status" aria-label={label} />;
}
export function Notice({ kind = "info", children }: { kind?: "info" | "success" | "error"; children: ReactNode }) {
  const Icon = kind === "success" ? Check : kind === "error" ? AlertCircle : Info;
  return <div className={`ax-notice ax-notice-${kind}`} role={kind === "error" ? "alert" : "status"}>
    <Icon size={16} aria-hidden="true" /><div>{children}</div>
  </div>;
}
export function ConfirmDialog({ title, children, confirmLabel, busy, onCancel, onConfirm }: {
  title: string; children: ReactNode; confirmLabel: string; busy: boolean;
  onCancel(): void; onConfirm(): void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const el = ref.current;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    el?.showModal();
    cancelRef.current?.focus();
    return () => { el?.close(); previous?.focus(); };
  }, []);
  return <dialog ref={ref} className="ax-dialog" aria-labelledby="confirm-title" onCancel={(event) => {
    event.preventDefault(); if (!busy) onCancel();
  }}>
    <button className="icon-button dialog-close" aria-label="Close confirmation" disabled={busy} onClick={onCancel}><X size={18} /></button>
    <div className="dialog-symbol"><AlertCircle size={22} /></div>
    <h2 id="confirm-title">{title}</h2>
    <div className="dialog-copy">{children}</div>
    <div className="dialog-actions">
      <button ref={cancelRef} className="text-button" disabled={busy} onClick={onCancel}>Cancel</button>
      <button className="danger-button" disabled={busy} onClick={onConfirm}>{busy && <Spinner />}{confirmLabel}</button>
    </div>
  </dialog>;
}

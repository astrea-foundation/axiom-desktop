import { Globe2 } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState } from "react";

export function WebConsentDialog({ onCancel, onConfirm }: {
  onCancel: () => void;
  onConfirm: (remember: boolean) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const decline = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const [remember, setRemember] = useState(false);
  useLayoutEffect(() => {
    dialog.current?.showModal();
    decline.current?.focus();
    return () => dialog.current?.close();
  }, []);

  const finish = (action: () => void) => {
    dialog.current?.close();
    action();
    document.querySelector<HTMLButtonElement>("[data-web-toggle]")?.focus({ preventScroll: true });
  };

  return <dialog
    ref={dialog}
    role="alertdialog"
    aria-modal="true"
    aria-labelledby={titleId}
    aria-describedby={descriptionId}
    onCancel={(event) => { event.preventDefault(); finish(onCancel); }}
    onKeyDown={(event) => {
      if (event.key !== "Tab") return;
      const controls = dialog.current?.querySelectorAll<HTMLElement>("input, button");
      const first = controls?.[0];
      const last = controls?.[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto w-[480px] max-w-[calc(100%_-_40px)] rounded-[22px] border-0 p-7 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]"
  >
    <Globe2 size={22} className="mb-4 text-[var(--color-warning)]" aria-hidden="true" />
    <h2 id={titleId} className="text-[20px] font-medium tracking-tight">Turn on Web?</h2>
    <div id={descriptionId} className="mt-3 space-y-3 text-[13.5px] leading-6 text-[var(--color-text-secondary)]">
      <p>Web searches run outside Axiom’s verified private environment. Queries are sent to web search providers to retrieve results.</p>
      <p>Your model conversation remains end-to-end encrypted, but search queries may contain details from your messages.</p>
    </div>
    <label className="mt-5 flex cursor-pointer items-center gap-2 text-[12.5px] text-[var(--color-text-secondary)]">
      <input type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} className="h-4 w-4 accent-[var(--color-cherry)]" />
      Don’t show this warning again
    </label>
    <div className="mt-6 flex flex-wrap justify-end gap-2">
      <button ref={decline} type="button" autoFocus onClick={() => finish(onCancel)} className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] hover:bg-[var(--wash-chip-hover)]">No thanks</button>
      <button type="button" onClick={() => finish(() => onConfirm(remember))} className="rounded-lg bg-[var(--color-cherry)] px-4 py-2 text-[13px] text-on-accent hover:bg-[var(--color-cherry-bright)]">Yes, I understand</button>
    </div>
  </dialog>;
}

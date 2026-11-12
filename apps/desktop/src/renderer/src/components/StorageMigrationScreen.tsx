import type { AccountStatus } from "@axiom/axiom-acp-client";
import { Check, LoaderCircle, LockKeyhole } from "lucide-react";
import { useEffect, useRef, useState } from "react";

type Progress = NonNullable<AccountStatus["storageMigration"]>;
const steps = [
  { phase: "encrypting", label: "Encrypting your chats" },
  { phase: "moving_media", label: "Moving images and files" },
  { phase: "checking", label: "Checking your saved data" },
  { phase: "finalizing", label: "Finishing up" },
] as const;

export function StorageMigrationScreen({ progress, detail, disconnected, onRetry }: {
  progress: Progress;
  detail?: string | null;
  disconnected: boolean;
  onRetry: () => Promise<void>;
}) {
  const [retrying, setRetrying] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  const failed = disconnected || progress.phase === "failed";
  const current = steps.findIndex(step => step.phase === progress.phase);
  const counted = Number.isSafeInteger(progress.completed) && Number.isSafeInteger(progress.total)
    && progress.total! > 0 && progress.completed! >= 0 && progress.completed! <= progress.total!;
  const percent = counted ? Math.round(progress.completed! / progress.total! * 100) : null;
  return (
    <main aria-labelledby="storage-migration-title"
      className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto rounded-[20px] bg-[var(--surface-card)] px-6 py-16">
      <div className="w-full max-w-[420px]">
        <div className="mb-7 flex h-12 w-12 items-center justify-center rounded-2xl bg-[var(--wash-hover)] text-[var(--color-text-secondary)]">
          <LockKeyhole size={23} strokeWidth={1.5} aria-hidden="true" />
        </div>
        <h1 ref={heading} tabIndex={-1} id="storage-migration-title" className="text-[23px] font-medium tracking-tight text-[var(--color-text-primary)] outline-none">
          {failed ? "Your storage update needs attention" : "Updating your local chat storage"}
        </h1>
        <p className="mt-3 text-[13px] leading-relaxed text-[var(--color-text-secondary)]">
          {failed ? "Axiom couldn’t finish the update. Your saved data has been kept so you can try again."
            : "Axiom is protecting your saved chats, images and files. Keep Axiom open while the update finishes. Your data stays on this device."}
        </p>
        {failed ? <>
          {detail ? <details className="mt-5 text-[12px] text-[var(--color-text-secondary)]">
            <summary className="cursor-pointer">Details</summary>
            <p className="mt-2 whitespace-pre-wrap break-words">{detail}</p>
          </details> : null}
          {disconnected ? <p role="status" className="mt-6 text-[13px] text-[var(--color-text-secondary)]">Waiting for Axiom to reconnect. The update will resume when it’s ready.</p> : <button type="button" disabled={retrying}
            onClick={() => { setRetrying(true); void onRetry().finally(() => setRetrying(false)); }}
            className="mt-6 rounded-full bg-[var(--surface-button-secondary)] px-4 py-2 text-[13px] text-[var(--color-text-primary)] disabled:opacity-50">
            {retrying ? "Trying again…" : "Try again"}
          </button>}
        </> : <>
          <ol className="mt-8 space-y-4" aria-label="Storage update steps">
            {steps.map((step, index) => <li key={step.phase} aria-current={index === current ? "step" : undefined}
              className={`flex items-center gap-3 text-[13px] ${index <= current ? "text-[var(--color-text-primary)]" : "text-[var(--color-text-tertiary)]"}`}>
              {index < current ? <Check size={16} aria-hidden="true" />
                : index === current ? <LoaderCircle size={16} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
                  : <span className="h-4 w-4 rounded-full border border-current opacity-40" aria-hidden="true" />}
              {step.label}
            </li>)}
          </ol>
          <div className="mt-7" role="status" aria-live="polite" aria-atomic="true">
            <p className="text-[12px] text-[var(--color-text-secondary)]">
              {steps[current]?.label ?? "Preparing your storage"}{counted ? ` · ${progress.completed} of ${progress.total} ${progress.phase === "moving_media" ? "saved records" : "files"}` : "…"}
            </p>
          </div>
          <div role="progressbar" aria-label="Current storage update step"
            aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}
            className="mt-3 h-1.5 overflow-hidden rounded-full bg-[var(--wash-hover)]">
            <div className={`h-full rounded-full bg-[var(--color-text-secondary)] ${percent === null ? "w-1/3 animate-pulse motion-reduce:animate-none" : "transition-[width]"}`}
              style={percent === null ? undefined : { width: `${percent}%` }} />
          </div>
          <p className="mt-4 text-[11.5px] text-[var(--color-text-tertiary)]">Large chat histories can take a little longer. This update only needs to finish once.</p>
        </>}
      </div>
    </main>
  );
}

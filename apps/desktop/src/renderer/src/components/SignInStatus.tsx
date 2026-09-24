import { Check, ExternalLink, LoaderCircle, X } from "lucide-react";
import type { SignInState } from "../signInFlow";

interface SignInStatusProps {
  state: Exclude<SignInState, { kind: "closed" }>;
  onCancel: () => void;
  onRetry: () => void;
}

export function SignInStatus({ state, onCancel, onRetry }: SignInStatusProps) {
  const actionClass = "inline-flex shrink-0 items-center gap-1.5 rounded-full bg-[var(--surface-button-secondary)] px-3 py-1.5 text-[11.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--surface-button-secondary-hover)]";

  return (
    <section
      aria-label="Browser sign-in"
      className="shadow-glass-card flex flex-wrap items-center gap-x-3 gap-y-2 rounded-[14px] bg-[var(--surface-card)] px-4 py-3"
    >
      <div role="status" className="flex min-w-0 flex-1 items-center gap-3">
        {state.kind === "success" ? (
          <Check size={16} className="shrink-0 text-[var(--color-success)]" aria-hidden="true" />
        ) : state.kind !== "error" ? (
          <LoaderCircle size={16} className="shrink-0 animate-spin text-[var(--color-text-tertiary)] motion-reduce:animate-none" aria-hidden="true" />
        ) : null}
        <div className="min-w-0">
          <p className="text-[13px] font-medium text-[var(--color-text-primary)]">
            {state.kind === "starting" ? "Opening your browser…"
              : state.kind === "waiting" ? "Continue in your browser"
                : state.kind === "success" ? "You’re signed in"
                  : "Sign-in didn’t finish"}
          </p>
          {state.kind === "waiting" ? (
            <p className="mt-1 text-[11.5px] text-[var(--color-text-secondary)]">
              Match this code:<span className="ml-1 whitespace-nowrap font-mono font-medium tracking-wider text-[var(--color-text-primary)]">{state.login.userCode}</span>
            </p>
          ) : state.kind === "error" ? (
            <p className="mt-1 break-words text-[11.5px] text-[var(--color-danger-strong)]">{state.message}</p>
          ) : null}
        </div>
      </div>
      {state.kind === "waiting" ? (
        <a href={state.login.authorizationUrl} target="_blank" rel="noreferrer" className={actionClass}>
          {state.login.browserOpened ? "Reopen browser" : "Open browser"}
          <ExternalLink size={12} aria-hidden="true" />
        </a>
      ) : state.kind === "error" ? (
        <button type="button" onClick={onRetry} className={actionClass}>Try again</button>
      ) : null}
      <button
        type="button"
        onClick={onCancel}
        aria-label={state.kind === "success" || state.kind === "error" ? "Dismiss sign-in status" : "Cancel sign-in"}
        className="shrink-0 rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]"
      >
        <X size={15} aria-hidden="true" />
      </button>
    </section>
  );
}

import { Check, Copy, ExternalLink, LoaderCircle, ShieldCheck, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { SignInState } from "../signInFlow";

interface SignInDialogProps {
  state: Exclude<SignInState, { kind: "closed" }>;
  onCancel: () => void;
  onRetry: () => void;
}

export function SignInDialog({ state, onCancel, onRetry }: SignInDialogProps) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    setCopied(false);
  }, [state.kind, state.kind === "waiting" ? state.login.userCode : null]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onCancel]);

  const waiting = state.kind === "waiting" ? state.login : null;

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-[var(--surface-scrim)] px-5 py-14">
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="sign-in-title"
        aria-describedby="sign-in-description"
        className="glass-panel-solid shadow-glass-pop animate-pop-in relative w-full max-w-[460px] overflow-hidden rounded-[22px]"
      >
        <div className="pointer-events-none absolute inset-x-12 top-0 h-24 rounded-full bg-[var(--color-cherry-glow)] blur-3xl" aria-hidden="true" />
        <button
          type="button"
          onClick={onCancel}
          className="absolute right-4 top-4 z-10 rounded-[6px] p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]"
          aria-label={state.kind === "success" ? "Close" : "Cancel sign-in"}
        >
          <X size={16} />
        </button>

        <div className="relative px-7 pb-7 pt-8 sm:px-8">
          <div
            className={[
              "mb-5 flex h-11 w-11 items-center justify-center rounded-[12px]",
              state.kind === "success"
                ? "bg-[var(--color-success-glow)] text-[var(--color-success)]"
                : state.kind === "error"
                  ? "bg-[var(--color-danger-soft)] text-[var(--color-danger-strong)]"
                  : "bg-[var(--wash-chip)] text-[var(--color-text-secondary)]",
            ].join(" ")}
          >
            {state.kind === "success" ? <Check size={21} strokeWidth={2.4} /> : <ShieldCheck size={21} strokeWidth={1.9} />}
          </div>

          <h2 id="sign-in-title" className="text-[20px] font-medium tracking-[-0.025em] text-[var(--color-text-primary)]">
            {state.kind === "starting"
              ? "Opening secure sign-in"
              : state.kind === "waiting"
                ? "Approve sign-in in your browser"
                : state.kind === "success"
                  ? "You’re signed in"
                  : "Sign-in didn’t finish"}
          </h2>

          <div id="sign-in-description" aria-live="polite" className="mt-2 text-[13.5px] leading-5 text-[var(--color-text-secondary)]">
            {state.kind === "starting" ? (
              <p>Axiom is creating a short-lived browser authorization.</p>
            ) : state.kind === "waiting" ? (
              <p>Confirm that the code below matches the Axiom approval page.</p>
            ) : state.kind === "success" ? (
              <p>Your account is connected. Loading your available models now.</p>
            ) : (
              <p>{state.message}</p>
            )}
          </div>

          {state.kind === "starting" ? (
            <div className="shadow-glass-tile mt-7 flex items-center gap-2.5 rounded-[10px] bg-[var(--surface-tile)] px-4 py-3 text-[12.5px] text-[var(--color-text-secondary)]">
              <LoaderCircle size={15} className="animate-spin text-[var(--color-cherry)]" />
              Starting browser authorization…
            </div>
          ) : null}

          {waiting ? (
            <>
              <div className="shadow-glass-tile mt-6 rounded-[14px] bg-[var(--surface-tile)] px-5 py-5 text-center">
                <div className="text-[10px] font-medium uppercase tracking-[0.18em] text-[var(--color-text-muted)]">Verification code</div>
                <div className="mt-2 font-mono text-[24px] font-medium tracking-[0.2em] text-[var(--color-text-primary)]">{waiting.userCode}</div>
              </div>
              <div className="mt-4 flex items-center gap-2 text-[12px] text-[var(--color-text-tertiary)]">
                <LoaderCircle size={13} className="animate-spin text-[var(--color-cherry)]" />
                Waiting for approval…
              </div>
              <div className="mt-6 flex flex-wrap gap-2">
                <a
                  href={waiting.authorizationUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-2 rounded-full bg-[var(--color-cherry)] px-4 py-2 text-[12.5px] font-medium text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.97]"
                >
                  <ExternalLink size={13} />
                  {waiting.browserOpened ? "Open browser again" : "Open browser"}
                </a>
                <button
                  type="button"
                  onClick={() => {
                    void navigator.clipboard.writeText(waiting.userCode).then(() => setCopied(true));
                  }}
                  className="inline-flex items-center gap-2 rounded-full bg-[var(--surface-button-secondary)] px-4 py-2 text-[12.5px] text-[var(--color-text-secondary)] shadow-[0_0_0_1px_var(--ring-hairline)] transition-colors hover:bg-[var(--surface-button-secondary-hover)] hover:text-[var(--color-text-primary)]"
                >
                  {copied ? <Check size={13} /> : <Copy size={13} />}
                  {copied ? "Copied" : "Copy code"}
                </button>
              </div>
            </>
          ) : null}

          {state.kind === "success" ? (
            <div className="mt-7 h-1 overflow-hidden rounded-full bg-[var(--surface-track)]">
              <div className="animate-progress-in h-full w-full origin-left rounded-full bg-[var(--color-success)]" />
            </div>
          ) : null}

          {state.kind === "error" ? (
            <div className="mt-7 flex gap-2">
              <button
                type="button"
                onClick={onRetry}
                className="rounded-full bg-[var(--color-cherry)] px-5 py-2 text-[12.5px] font-medium text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.97]"
              >
                Try again
              </button>
              <button
                type="button"
                onClick={onCancel}
                className="rounded-full bg-[var(--surface-button-secondary)] px-5 py-2 text-[12.5px] text-[var(--color-text-secondary)] shadow-[0_0_0_1px_var(--ring-hairline)] transition-colors hover:bg-[var(--surface-button-secondary-hover)] hover:text-[var(--color-text-primary)]"
              >
                Not now
              </button>
            </div>
          ) : state.kind !== "success" && !waiting ? (
            <button
              type="button"
              onClick={onCancel}
              className="mt-6 text-[12px] text-[var(--color-text-tertiary)] transition-colors hover:text-[var(--color-text-secondary)]"
            >
              Cancel
            </button>
          ) : waiting ? (
            <button
              type="button"
              onClick={onCancel}
              className="mt-5 text-[12px] text-[var(--color-text-tertiary)] transition-colors hover:text-[var(--color-text-secondary)]"
            >
              Cancel sign-in
            </button>
          ) : null}
        </div>
      </section>
    </div>
  );
}

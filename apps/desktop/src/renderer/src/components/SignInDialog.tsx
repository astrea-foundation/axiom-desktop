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
    <div className="absolute inset-0 z-[70] flex items-center justify-center bg-black/65 px-5 py-14 backdrop-blur-md">
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="sign-in-title"
        aria-describedby="sign-in-description"
        className="animate-pop-in relative w-full max-w-[460px] overflow-hidden rounded-[22px] border border-white/[0.09] bg-[var(--color-bg-surface)] shadow-[0_30px_90px_rgba(0,0,0,0.6)]"
      >
        <div className="pointer-events-none absolute inset-x-12 top-0 h-24 rounded-full bg-[var(--color-cherry)]/[0.08] blur-3xl" aria-hidden="true" />
        <button
          type="button"
          onClick={onCancel}
          className="absolute right-4 top-4 z-10 rounded-lg p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-white/[0.05] hover:text-[var(--color-text-primary)]"
          aria-label={state.kind === "success" ? "Close" : "Cancel sign-in"}
        >
          <X size={16} />
        </button>

        <div className="relative px-7 pb-7 pt-8 sm:px-8">
          <div className="mb-5 flex h-11 w-11 items-center justify-center rounded-2xl border border-[var(--color-border-accent)] bg-[var(--color-cherry-glow)] text-[var(--color-cherry-bright)] shadow-[0_0_28px_rgba(255,77,109,0.12)]">
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
            <div className="mt-7 flex items-center gap-2.5 rounded-xl border border-white/[0.055] bg-[var(--color-bg-input)] px-4 py-3 text-[12.5px] text-[var(--color-text-secondary)]">
              <LoaderCircle size={15} className="animate-spin text-[var(--color-cherry)]" />
              Starting browser authorization…
            </div>
          ) : null}

          {waiting ? (
            <>
              <div className="mt-6 rounded-2xl border border-[var(--color-border-accent)] bg-[var(--color-bg-input)] px-5 py-5 text-center shadow-[inset_0_0_28px_rgba(255,77,109,0.025)]">
                <div className="text-[10px] font-medium uppercase tracking-[0.18em] text-[var(--color-text-muted)]">Verification code</div>
                <div className="mt-2 font-mono text-[24px] font-medium tracking-[0.2em] text-[var(--color-cherry-bright)]">{waiting.userCode}</div>
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
                  className="inline-flex items-center gap-2 rounded-full bg-[var(--color-cherry)] px-4 py-2 text-[12.5px] font-medium text-[#140709] transition-colors hover:bg-[var(--color-cherry-bright)]"
                >
                  <ExternalLink size={13} />
                  {waiting.browserOpened ? "Open browser again" : "Open browser"}
                </a>
                <button
                  type="button"
                  onClick={() => {
                    void navigator.clipboard.writeText(waiting.userCode).then(() => setCopied(true));
                  }}
                  className="inline-flex items-center gap-2 rounded-full border border-[var(--color-border)] px-4 py-2 text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:border-white/[0.13] hover:bg-white/[0.035] hover:text-[var(--color-text-primary)]"
                >
                  {copied ? <Check size={13} /> : <Copy size={13} />}
                  {copied ? "Copied" : "Copy code"}
                </button>
              </div>
            </>
          ) : null}

          {state.kind === "success" ? (
            <div className="mt-7 h-1 overflow-hidden rounded-full bg-white/[0.05]">
              <div className="h-full w-full origin-left animate-[progress-in_700ms_ease-out] rounded-full bg-[var(--color-cherry)]" />
            </div>
          ) : null}

          {state.kind === "error" ? (
            <div className="mt-7 flex gap-2">
              <button
                type="button"
                onClick={onRetry}
                className="rounded-full bg-[var(--color-cherry)] px-5 py-2 text-[12.5px] font-medium text-[#140709] transition-colors hover:bg-[var(--color-cherry-bright)]"
              >
                Try again
              </button>
              <button
                type="button"
                onClick={onCancel}
                className="rounded-full border border-[var(--color-border)] px-5 py-2 text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:text-[var(--color-text-primary)]"
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

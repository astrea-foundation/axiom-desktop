import { Check, LoaderCircle, RotateCcw, Shield, X } from "lucide-react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";

type TeeVerificationBadgeProps = {
  security: ClientSessionState["security"];
  onVerify: () => void;
};

type Glyph = "check" | "x" | null;

export function TeeVerificationBadge({ security, onVerify }: TeeVerificationBadgeProps) {
  const state = security?.state ?? "unverified";
  const presentation: { label: string; detail: string; shell: string; fill: string; glyph: Glyph } = state === "verified"
    ? {
        label: "TEE verified",
        detail: security?.detail ?? "The model's confidential-computing evidence was verified locally.",
        shell: "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface-hover)]",
        fill: "var(--color-success)",
        glyph: "check",
      }
    : state === "verifying"
      ? {
          label: "Verifying TEE",
          detail: security?.detail ?? "Checking fresh confidential-computing evidence.",
          shell: "text-[var(--color-text-secondary)]",
          fill: "",
          glyph: null,
        }
      : state === "failed"
        ? {
            label: "TEE check failed",
            detail: security?.detail ?? "The model's confidential-computing evidence could not be verified.",
            shell: "text-[var(--color-danger-strong)] hover:bg-[var(--color-danger-soft)]",
            fill: "var(--color-danger)",
            glyph: "x",
          }
        : state === "unattested_development"
          ? {
              label: "Unattested dev",
              detail: security?.detail ?? "This development session is not backed by production TEE attestation.",
              shell: "text-[var(--color-text-secondary)] hover:bg-warning-soft",
              fill: "var(--color-warning)",
              glyph: null,
            }
          : {
              label: "Verify TEE",
              detail: security?.detail ?? "Fresh confidential-computing evidence has not been verified yet.",
              shell: "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface-hover)]",
              fill: "var(--color-text-muted)",
              glyph: null,
            };
  const verifying = state === "verifying";

  return (
    <button
      type="button"
      onClick={onVerify}
      disabled={verifying}
      className={`group/tee inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-[var(--surface-tile)] pl-2 pr-2.5 text-[11.5px] font-medium tracking-[-0.01em] shadow-[0_0_0_1px_var(--ring-hairline),0_1px_2px_var(--shade-xs)] transition-all disabled:cursor-wait ${presentation.shell}`}
      title={`${presentation.detail}${verifying ? "" : " Click to verify again."}`}
      aria-label={`${presentation.label}. ${presentation.detail}${verifying ? "" : " Activate to verify again."}`}
      aria-live="polite"
    >
      <span className="relative flex h-4 w-4 items-center justify-center">
        {verifying ? (
          <LoaderCircle size={15} strokeWidth={1.9} className="animate-spin text-[var(--color-text-tertiary)]" />
        ) : (
          <>
            {/* Solid silhouette: no stroke, so no outline on any background. */}
            <Shield size={15} strokeWidth={0} stroke="none" fill="currentColor" style={{ color: presentation.fill }} />
            {presentation.glyph === "check" ? (
              <Check size={8} strokeWidth={3.5} className="absolute text-on-accent" />
            ) : presentation.glyph === "x" ? (
              <X size={8} strokeWidth={3.5} className="absolute text-on-accent" />
            ) : null}
          </>
        )}
      </span>
      <span className="flex items-center">
        {presentation.label}
        {!verifying ? (
          <RotateCcw
            size={10.5}
            className="ml-0 w-0 shrink-0 opacity-0 transition-[width,margin,opacity] duration-200 group-hover/tee:ml-1.5 group-hover/tee:w-[10.5px] group-hover/tee:opacity-60 group-focus-visible/tee:ml-1.5 group-focus-visible/tee:w-[10.5px] group-focus-visible/tee:opacity-60"
          />
        ) : null}
      </span>
    </button>
  );
}

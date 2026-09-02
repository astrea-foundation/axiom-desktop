import {
  LoaderCircle,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  ShieldQuestion,
} from "lucide-react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";

type TeeVerificationBadgeProps = {
  security: ClientSessionState["security"];
  onVerify: () => void;
};

export function TeeVerificationBadge({ security, onVerify }: TeeVerificationBadgeProps) {
  const state = security?.state ?? "unverified";
  const presentation = state === "verified"
    ? {
        label: "TEE verified",
        detail: security?.detail ?? "The model's confidential-computing evidence was verified locally.",
        icon: ShieldCheck,
        shell: "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface-hover)]",
        iconTone: "text-[var(--color-cherry)]",
        dot: "bg-[var(--color-success)] shadow-[0_0_7px_rgba(30,127,76,0.45)]",
      }
    : state === "verifying"
      ? {
          label: "Verifying TEE",
          detail: security?.detail ?? "Checking fresh confidential-computing evidence.",
          icon: LoaderCircle,
          shell: "text-[var(--color-text-secondary)]",
          iconTone: "text-[var(--color-text-tertiary)]",
          dot: "bg-[var(--color-cherry)]",
        }
      : state === "failed"
        ? {
            label: "TEE check failed",
            detail: security?.detail ?? "The model's confidential-computing evidence could not be verified.",
            icon: ShieldAlert,
            shell: "text-[var(--color-danger-strong)] hover:bg-[var(--color-danger-soft)]",
            iconTone: "text-[var(--color-danger)]",
            dot: "bg-[var(--color-danger)] shadow-[0_0_7px_rgba(217,48,54,0.4)]",
          }
        : state === "unattested_development"
          ? {
              label: "Unattested dev",
              detail: security?.detail ?? "This development session is not backed by production TEE attestation.",
              icon: ShieldAlert,
              shell: "text-[var(--color-text-secondary)] hover:bg-[#fdf6e8]",
              iconTone: "text-amber-600",
              dot: "bg-amber-500 shadow-[0_0_7px_rgba(217,119,6,0.35)]",
            }
          : {
              label: "Verify TEE",
              detail: security?.detail ?? "Fresh confidential-computing evidence has not been verified yet.",
              icon: ShieldQuestion,
              shell: "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface-hover)]",
              iconTone: "text-[var(--color-text-tertiary)]",
              dot: "bg-[var(--color-text-muted)]",
            };
  const Icon = presentation.icon;
  const verifying = state === "verifying";

  return (
    <button
      type="button"
      onClick={onVerify}
      disabled={verifying}
      className={`group inline-flex h-7 shrink-0 items-center gap-2 rounded-full bg-white px-2.5 text-[11.5px] font-medium tracking-[-0.01em] shadow-[0_0_0_1px_rgba(16,16,20,0.045),0_1px_2px_rgba(16,16,20,0.03)] transition-all disabled:cursor-wait ${presentation.shell}`}
      title={`${presentation.detail}${verifying ? "" : " Click to verify again."}`}
      aria-label={`${presentation.label}. ${presentation.detail}${verifying ? "" : " Activate to verify again."}`}
      aria-live="polite"
    >
      <span className="relative flex h-4 w-4 items-center justify-center">
        <Icon size={14} strokeWidth={1.9} className={`${presentation.iconTone} ${verifying ? "animate-spin" : ""}`} />
        <span className={`absolute -bottom-0.5 -right-0.5 h-1.5 w-1.5 rounded-full ring-2 ring-white ${presentation.dot}`} />
      </span>
      <span>{presentation.label}</span>
      {!verifying ? <RotateCcw size={10.5} className="ml-0.5 opacity-0 transition-opacity group-hover:opacity-60 group-focus-visible:opacity-60" /> : null}
    </button>
  );
}

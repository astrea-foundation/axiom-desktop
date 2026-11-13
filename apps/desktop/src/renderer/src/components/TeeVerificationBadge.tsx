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
        shell: "border-[var(--color-cherry)]/20 bg-[var(--color-cherry)]/[0.055] text-[var(--color-text-secondary)] hover:border-[var(--color-cherry)]/30 hover:bg-[var(--color-cherry)]/[0.085]",
        iconTone: "text-[var(--color-cherry)]",
        dot: "bg-[var(--color-cherry)] shadow-[0_0_7px_rgba(255,77,109,0.62)]",
      }
    : state === "verifying"
      ? {
          label: "Verifying TEE",
          detail: security?.detail ?? "Checking fresh confidential-computing evidence.",
          icon: LoaderCircle,
          shell: "border-[var(--color-cherry)]/18 bg-[var(--color-cherry)]/[0.045] text-[var(--color-text-secondary)]",
          iconTone: "text-[var(--color-cherry-soft)]",
          dot: "bg-[var(--color-cherry-soft)] shadow-[0_0_7px_rgba(230,61,91,0.55)]",
        }
      : state === "failed"
        ? {
            label: "TEE check failed",
            detail: security?.detail ?? "The model's confidential-computing evidence could not be verified.",
            icon: ShieldAlert,
            shell: "border-red-400/18 bg-red-400/[0.05] text-[var(--color-text-secondary)] hover:border-red-400/28 hover:bg-red-400/[0.075]",
            iconTone: "text-red-300",
            dot: "bg-red-400 shadow-[0_0_7px_rgba(248,113,113,0.48)]",
          }
        : state === "unattested_development"
          ? {
              label: "Unattested dev",
              detail: security?.detail ?? "This development session is not backed by production TEE attestation.",
              icon: ShieldAlert,
              shell: "border-amber-300/15 bg-amber-300/[0.045] text-[var(--color-text-secondary)] hover:border-amber-300/25",
              iconTone: "text-amber-300/80",
              dot: "bg-amber-300/80 shadow-[0_0_7px_rgba(252,211,77,0.4)]",
            }
          : {
              label: "Verify TEE",
              detail: security?.detail ?? "Fresh confidential-computing evidence has not been verified yet.",
              icon: ShieldQuestion,
              shell: "border-white/[0.09] bg-white/[0.045] text-[var(--color-text-secondary)] hover:border-white/[0.16] hover:bg-white/[0.07]",
              iconTone: "text-[var(--color-text-tertiary)]",
              dot: "bg-[var(--color-text-tertiary)]",
            };
  const Icon = presentation.icon;
  const verifying = state === "verifying";

  return (
    <button
      type="button"
      onClick={onVerify}
      disabled={verifying}
      className={`group inline-flex h-7 shrink-0 items-center gap-2 rounded-full border px-2.5 text-[11.5px] font-medium tracking-[-0.01em] shadow-[0_6px_18px_rgba(0,0,0,0.14)] backdrop-blur-md transition-all disabled:cursor-wait ${presentation.shell}`}
      title={`${presentation.detail}${verifying ? "" : " Click to verify again."}`}
      aria-label={`${presentation.label}. ${presentation.detail}${verifying ? "" : " Activate to verify again."}`}
      aria-live="polite"
    >
      <span className="relative flex h-4 w-4 items-center justify-center">
        <Icon size={14} strokeWidth={1.9} className={`${presentation.iconTone} ${verifying ? "animate-spin" : ""}`} />
        <span className={`absolute -bottom-0.5 -right-0.5 h-1.5 w-1.5 rounded-full ring-2 ring-[var(--color-bg-input)] ${presentation.dot}`} />
      </span>
      <span>{presentation.label}</span>
      {!verifying ? <RotateCcw size={10.5} className="ml-0.5 opacity-0 transition-opacity group-hover:opacity-60 group-focus-visible:opacity-60" /> : null}
    </button>
  );
}

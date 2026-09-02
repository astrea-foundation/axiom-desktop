import { Cpu, EyeOff, LogIn, RefreshCw, ShieldAlert, ShieldCheck } from "lucide-react";
import { AxiomMark } from "../brand/AxiomLogo";
import { GreetingHeadline } from "../brand/GreetingHeadline";
import type { AccountPresentation } from "../signInFlow";
import type { ProviderModel, ReasoningEffort } from "../types";
import { ChatComposer } from "./ChatComposer";

interface WelcomeViewProps {
  draft: string;
  onDraftChange: (value: string) => void;
  onSubmit: () => void;
  models: ProviderModel[];
  selectedModelId: string;
  modelStatusLabel: string;
  onModelChange: (modelId: string) => void;
  account: AccountPresentation;
  onSignIn: () => void;
  onRefreshAccount: () => void;
  reasoningEffort: ReasoningEffort;
  onReasoningEffortChange: (effort: ReasoningEffort) => void;
}

export function WelcomeView({
  draft,
  onDraftChange,
  onSubmit,
  models,
  selectedModelId,
  modelStatusLabel,
  onModelChange,
  account,
  onSignIn,
  onRefreshAccount,
  reasoningEffort,
  onReasoningEffortChange,
}: WelcomeViewProps) {
  return (
    <div className="relative flex h-full w-full items-center justify-center px-6">
      {/* A fixed 2000×1400 composition pinned to the view's centre: the window
          reveals more or less of it instead of moving the blooms around. */}
      <div className="pointer-events-none absolute inset-0 overflow-hidden" aria-hidden="true">
        <div className="absolute left-1/2 top-1/2 h-[1400px] w-[2000px] -translate-x-1/2 -translate-y-1/2">
          <div
            className="bg-blob"
            style={{ left: 120, top: 250, width: 780, height: 640, borderRadius: "62% 38% 55% 45% / 48% 57% 43% 52%", transform: "rotate(-14deg)" }}
          />
          <div
            className="bg-blob"
            style={{ left: 1190, top: 130, width: 720, height: 560, borderRadius: "41% 59% 47% 53% / 62% 36% 64% 38%", transform: "rotate(23deg)", opacity: 0.8 }}
          />
          <div
            className="bg-blob"
            style={{ left: 640, top: 830, width: 900, height: 620, borderRadius: "56% 44% 63% 37% / 39% 55% 45% 61%", transform: "rotate(8deg)", opacity: 0.7 }}
          />
        </div>
      </div>

      <div className="relative z-10 flex w-full max-w-[760px] flex-col items-center animate-fade-in">
        <AxiomMark className="mb-7 h-10 w-auto" />
        <GreetingHeadline />
        {account.kind !== "valid" && account.kind !== "starting" ? (
          <div className="shadow-glass-card mb-3 flex w-full items-center gap-3 rounded-[14px] bg-[var(--surface-card)] px-4 py-3">
            <div
              className={[
                "flex h-9 w-9 shrink-0 items-center justify-center rounded-[10px]",
                account.kind === "unavailable"
                  ? "bg-[var(--color-warning-soft)] text-[var(--color-warning)]"
                  : "bg-[var(--wash-chip)] text-[var(--color-text-secondary)]",
              ].join(" ")}
            >
              {account.kind === "unavailable" ? <ShieldAlert size={17} /> : <LogIn size={17} />}
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium text-[var(--color-text-primary)]">{account.title}</div>
              <div className="mt-0.5 truncate text-[11.5px] text-[var(--color-text-tertiary)]">{account.detail}</div>
            </div>
            <button
              type="button"
              onClick={account.kind === "unavailable" ? onRefreshAccount : onSignIn}
              className="flex shrink-0 items-center gap-1.5 rounded-full bg-[var(--color-cherry)] px-3.5 py-1.5 text-[11.5px] font-medium text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] hover:bg-[var(--color-cherry-bright)] active:scale-[0.97]"
            >
              {account.kind === "unavailable" ? <RefreshCw size={12} /> : <LogIn size={12} />}
              {account.kind === "unavailable" ? "Try again" : account.kind === "invalid" ? "Sign in again" : "Continue in browser"}
            </button>
          </div>
        ) : null}
        <div className="w-full">
          <ChatComposer
            value={draft}
            onChange={onDraftChange}
            onSubmit={onSubmit}
            autoFocus
            models={models}
            selectedModelId={selectedModelId}
            modelStatusLabel={modelStatusLabel}
            onModelChange={onModelChange}
            requiresSignIn={account.kind === "signed-out" || account.kind === "invalid"}
            onSignIn={onSignIn}
            reasoningEffort={reasoningEffort}
            onReasoningEffortChange={onReasoningEffortChange}
          />
        </div>
        <div className="mt-3.5 flex flex-wrap items-center justify-center gap-[18px] text-[11.5px] text-[var(--color-text-tertiary)]">
          <span className="inline-flex items-center gap-1.5">
            <ShieldCheck size={12} className="shrink-0 text-[var(--color-text-muted)]" />
            Sealed on your machine
          </span>
          <span className="inline-flex items-center gap-1.5">
            <Cpu size={12} className="shrink-0 text-[var(--color-text-muted)]" />
            Attested enclave
          </span>
          <span className="inline-flex items-center gap-1.5">
            <EyeOff size={12} className="shrink-0 text-[var(--color-text-muted)]" />
            0 bytes plaintext out
          </span>
        </div>
      </div>
    </div>
  );
}

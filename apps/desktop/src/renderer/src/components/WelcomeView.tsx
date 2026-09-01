import { LogIn, RefreshCw, ShieldAlert } from "lucide-react";
import { useRef } from "react";
import { CherryBlossomBackground } from "../brand/CherryBlossomBackground";
import { CornerEmberBackground } from "../brand/CornerEmberBackground";
import { GreetingHeadline } from "../brand/GreetingHeadline";
import { InputFrost } from "../brand/InputFrost";
import type { ProviderModel, ReasoningEffort } from "../types";
import type { AccountPresentation } from "../signInFlow";
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
  const welcomeRef = useRef<HTMLDivElement>(null);
  const inputAreaRef = useRef<HTMLDivElement>(null);

  return (
    <div ref={welcomeRef} className="relative flex h-full w-full items-center justify-center px-6">
      <CornerEmberBackground />
      <CherryBlossomBackground />
      <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
        <div className="cherry-glow animate-pulse-glow h-[640px] w-[840px] max-w-full rounded-full" />
      </div>
      <InputFrost targetRef={inputAreaRef} containerRef={welcomeRef} />

      <div className="relative z-10 flex w-full max-w-[760px] flex-col items-center animate-fade-in">
        <GreetingHeadline />
        {account.kind !== "valid" && account.kind !== "starting" ? (
          <div className="mb-3 flex w-full items-center gap-3 rounded-2xl border border-white/[0.065] bg-[var(--color-bg-surface)]/75 px-4 py-3 shadow-lg shadow-black/10 backdrop-blur-xl">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl border border-[var(--color-border-accent)] bg-[var(--color-cherry-glow)] text-[var(--color-cherry-bright)]">
              {account.kind === "unavailable" ? <ShieldAlert size={17} /> : <LogIn size={17} />}
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium text-[var(--color-text-primary)]">{account.title}</div>
              <div className="mt-0.5 truncate text-[11.5px] text-[var(--color-text-tertiary)]">{account.detail}</div>
            </div>
            <button
              type="button"
              onClick={account.kind === "unavailable" ? onRefreshAccount : onSignIn}
              className="flex shrink-0 items-center gap-1.5 rounded-full bg-[var(--color-cherry)] px-3.5 py-1.5 text-[11.5px] font-medium text-[#140709] transition-colors hover:bg-[var(--color-cherry-bright)]"
            >
              {account.kind === "unavailable" ? <RefreshCw size={12} /> : <LogIn size={12} />}
              {account.kind === "unavailable" ? "Try again" : account.kind === "invalid" ? "Sign in again" : "Continue in browser"}
            </button>
          </div>
        ) : null}
        <div ref={inputAreaRef} className="w-full">
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
      </div>
    </div>
  );
}

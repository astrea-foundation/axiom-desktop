import { Cpu, EyeOff, ShieldCheck } from "lucide-react";
import { AxiomMark } from "../brand/AxiomLogo";
import { GreetingHeadline } from "../brand/GreetingHeadline";
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
  reasoningEffort,
  onReasoningEffortChange,
}: WelcomeViewProps) {
  return (
    <div className="relative flex h-full w-full items-center justify-center px-6">
      <div className="pointer-events-none absolute inset-0 overflow-hidden" aria-hidden="true">
        <div className="bg-orb -left-[140px] top-[16%] h-[560px] w-[560px]" />
        <div className="bg-orb -right-[120px] top-[8%] h-[480px] w-[480px] opacity-70" />
        <div className="bg-orb bottom-[-160px] left-[38%] h-[620px] w-[620px] opacity-60" />
      </div>

      <div className="relative z-10 flex w-full max-w-[760px] flex-col items-center animate-fade-in">
        <AxiomMark className="mb-7 h-10 w-auto" />
        <GreetingHeadline />
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

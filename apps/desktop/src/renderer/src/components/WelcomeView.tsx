import { useRef } from "react";
import { CherryBlossomBackground } from "../brand/CherryBlossomBackground";
import { CornerEmberBackground } from "../brand/CornerEmberBackground";
import { GreetingHeadline } from "../brand/GreetingHeadline";
import { InputFrost } from "../brand/InputFrost";
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
            reasoningEffort={reasoningEffort}
            onReasoningEffortChange={onReasoningEffortChange}
          />
        </div>
      </div>
    </div>
  );
}

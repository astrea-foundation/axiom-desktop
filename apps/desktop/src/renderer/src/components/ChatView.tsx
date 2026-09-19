import { ArrowDown } from "lucide-react";
import type { ReactNode } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { PendingUserMessage, ProviderModel, ReasoningEffort } from "../types";
import { ChatComposer } from "./ChatComposer";
import type { AgentControlOptions } from "./AgentModeControl";
import { TimelineView } from "./TimelineView";
import { useChatAutoScroll } from "../useChatAutoScroll";

interface ChatViewProps {
  agentControl?: AgentControlOptions;
  queue?: ReactNode;
  session: ClientSessionState;
  draft: string;
  onDraftChange: (value: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  onRevise?: (userItemId: string, text: string) => Promise<void>;
  isStreaming: boolean;
  models: ProviderModel[];
  selectedModelId: string;
  modelStatusLabel: string;
  onModelChange: (modelId: string) => void;
  requiresSignIn: boolean;
  onSignIn: () => void;
  reasoningEffort: ReasoningEffort;
  onReasoningEffortChange: (effort: ReasoningEffort) => void;
  pendingUserMessage?: PendingUserMessage | null;
  onVerifySecurity: () => void | Promise<unknown>;
  securityScope?: string;
  securityRefreshDisabledReason?: string;
  webEnabled: boolean;
  webDisabled: boolean;
  onWebToggle: () => void;
}

export function ChatView({
  agentControl,
  queue,
  session,
  draft,
  onDraftChange,
  onSubmit,
  onStop,
  onRevise,
  isStreaming,
  models,
  selectedModelId,
  modelStatusLabel,
  onModelChange,
  requiresSignIn,
  onSignIn,
  reasoningEffort,
  onReasoningEffortChange,
  pendingUserMessage,
  onVerifySecurity,
  securityScope,
  securityRefreshDisabledReason,
  webEnabled,
  webDisabled,
  onWebToggle,
}: ChatViewProps) {
  const autoScroll = useChatAutoScroll(session.sessionId);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
      <div
        ref={autoScroll.viewportRef}
        data-chat-scroll
        role="region"
        aria-label="Chat messages"
        tabIndex={0}
        className="title-fade h-full overflow-y-auto px-6"
        style={{ overflowAnchor: "none" }}
        {...autoScroll.handlers}
      >
        <div ref={autoScroll.contentRef} className="mx-auto flex w-full max-w-[760px] flex-col pb-8 pt-[88px]">
          <TimelineView session={session} pendingUserMessage={pendingUserMessage} onRevise={onRevise} />
        </div>
      </div>

      {autoScroll.showJump ? (
        <button
          type="button"
          onClick={autoScroll.resume}
          aria-label="Scroll to latest"
          className="shadow-glass-pop animate-fade-in absolute bottom-3 left-1/2 z-10 flex h-9 w-9 -translate-x-1/2 items-center justify-center rounded-full bg-[var(--surface-float)] text-[var(--color-text-secondary)] transition-[transform,color] hover:text-[var(--color-text-primary)] active:scale-90"
        >
          <ArrowDown size={16} />
        </button>
      ) : null}
      </div>

      <div className="shrink-0 px-6 pb-5 pt-2">
        <div className="mx-auto w-full max-w-[760px]">
          {queue}
          <ChatComposer
            agentControl={agentControl}
            webEnabled={webEnabled}
            webDisabled={webDisabled}
            onWebToggle={onWebToggle}
            value={draft}
            onChange={onDraftChange}
            onSubmit={onSubmit}
            onStop={onStop}
            isStreaming={isStreaming}
            models={models}
            selectedModelId={selectedModelId}
            modelStatusLabel={modelStatusLabel}
            onModelChange={onModelChange}
            requiresSignIn={requiresSignIn}
            onSignIn={onSignIn}
            reasoningEffort={reasoningEffort}
            onReasoningEffortChange={onReasoningEffortChange}
            security={session}
            securityScope={securityScope}
            securityRefreshDisabledReason={securityRefreshDisabledReason}
            contextUsage={session.contextUsage ?? null}
            requestUsage={session.requestUsage}
            activeTurnId={session.activeTurnId}
            onVerifySecurity={onVerifySecurity}
          />
        </div>
      </div>
    </div>
  );
}

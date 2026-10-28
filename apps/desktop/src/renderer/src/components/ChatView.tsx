import { ArrowDown } from "lucide-react";
import type { ReactNode } from "react";
import type { ClientSessionState, PromptAttachment } from "@axiom/axiom-acp-client";
import type { AttachmentPreviews } from "../attachmentPreviews";
import type { PendingUserMessage, ProviderModel, ReasoningEffort } from "../types";
import type { AttachmentControl } from "./Attachments";
import { ChatComposer } from "./ChatComposer";
import type { AgentControlOptions } from "./AgentModeControl";
import { TimelineView } from "./TimelineView";
import { PreparingThreadView } from "./PreparingThreadView";
import { useChatAutoScroll } from "../useChatAutoScroll";
import { OutdatedTeeNotice } from "./OutdatedTeeNotice";
import { outdatedTeeState } from "../privacyProof";

interface ChatViewProps {
  attachments?: AttachmentControl;
  agentControl?: AgentControlOptions;
  queue?: ReactNode;
  session: ClientSessionState | null;
  preparation?: { id: string; text: string; attachments: PromptAttachment[] } | null;
  attachmentPreviews?: AttachmentPreviews;
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
  onAcceptOutdatedTee?: () => Promise<void>;
  securityScope?: string;
  securityRefreshDisabledReason?: string;
  webEnabled: boolean;
  webDisabled: boolean;
  onWebToggle: () => void;
}

export function ChatView({
  attachments,
  agentControl,
  queue,
  session,
  preparation,
  attachmentPreviews,
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
  onAcceptOutdatedTee,
  securityScope,
  securityRefreshDisabledReason,
  webEnabled,
  webDisabled,
  onWebToggle,
}: ChatViewProps) {
  const autoScroll = useChatAutoScroll(preparation?.id ?? session?.sessionId ?? "");

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
      {preparation ? <PreparingThreadView {...preparation} /> : session ? <>
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
          <TimelineView key={securityScope} session={session} pendingUserMessage={pendingUserMessage} onRevise={onRevise} attachmentPreviews={attachmentPreviews} />
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
      </> : null}
      </div>

      <div className="shrink-0 px-6 pb-5 pt-2">
        <div className="mx-auto w-full max-w-[760px]">
          {!preparation && onAcceptOutdatedTee ? <OutdatedTeeNotice key={`${securityScope}:${session?.sessionId}:${selectedModelId}`} state={outdatedTeeState(session, selectedModelId)}
            disabled={isStreaming || !!securityRefreshDisabledReason} onContinue={onAcceptOutdatedTee} /> : null}
          {!preparation && queue}
          <ChatComposer
            preparing={!!preparation}
            attachments={attachments}
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
            security={preparation ? undefined : session ?? undefined}
            securityScope={securityScope}
            securityRefreshDisabledReason={securityRefreshDisabledReason}
            contextUsage={session?.contextUsage ?? null}
            requestUsage={session?.requestUsage}
            activeTurnId={session?.activeTurnId}
            onVerifySecurity={onVerifySecurity}
          />
        </div>
      </div>
    </div>
  );
}

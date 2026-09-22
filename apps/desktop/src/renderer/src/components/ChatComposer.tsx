import { acceptsAttachment, attachmentAccept } from "../attachments";
import {
  ArrowUp,
  BrainCog,
  Check,
  ChevronDown,
  Globe2,
  Lock,
  Paperclip,
  Square,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { ProviderModel, ReasoningEffort } from "../types";
import { explicitReasoningEfforts, reconcileReasoningEffort } from "../reasoningSettings";
import { ContextUsageIndicator } from "./ContextUsageIndicator";
import { ModelBrandIcon, ProviderBrandIcon } from "./ModelBrandIcon";
import { ModelPicker } from "./ModelPicker";
import { TeeVerificationBadge } from "./TeeVerificationBadge";
import { DraftAttachments, type AttachmentControl } from "./Attachments";
import { AgentModeControl, type AgentControlOptions } from "./AgentModeControl";

function reasoningEffortLabel(effort: ReasoningEffort): string {
  if (effort === "enabled") return "On";
  if (effort === "disabled") return "Off";
  if (effort === "xhigh") return "Extra high";
  return effort[0]?.toUpperCase() + effort.slice(1);
}

type OpenMenu = "model" | "thinking" | "agent" | null;

interface ChatComposerProps {
  attachments?: AttachmentControl;
  agentControl?: AgentControlOptions;
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onStop?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
  preparing?: boolean;
  isStreaming?: boolean;
  models: ProviderModel[];
  selectedModelId: string;
  modelStatusLabel: string;
  onModelChange: (modelId: string) => void;
  requiresSignIn?: boolean;
  onSignIn?: () => void;
  reasoningEffort: ReasoningEffort;
  onReasoningEffortChange: (effort: ReasoningEffort) => void;
  security?: ClientSessionState;
  securityScope?: string;
  securityRefreshDisabledReason?: string;
  contextUsage?: ClientSessionState["contextUsage"];
  requestUsage?: ClientSessionState["requestUsage"];
  activeTurnId?: string | null;
  onVerifySecurity?: () => void | Promise<unknown>;
  webEnabled?: boolean;
  webDisabled?: boolean;
  onWebToggle?: () => void;
}

export function ChatComposer({
  attachments,
  agentControl,
  value,
  onChange,
  onSubmit,
  onStop,
  placeholder = "Ask anything",
  autoFocus = false,
  preparing = false,
  isStreaming = false,
  models,
  selectedModelId,
  modelStatusLabel,
  onModelChange,
  requiresSignIn = false,
  onSignIn,
  reasoningEffort,
  onReasoningEffortChange,
  security,
  securityScope,
  securityRefreshDisabledReason,
  contextUsage = null,
  requestUsage = [],
  activeTurnId,
  onVerifySecurity,
  webEnabled = false,
  webDisabled = false,
  onWebToggle,
}: ChatComposerProps) {
  const fileInput = useRef<HTMLInputElement>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const controlsRef = useRef<HTMLDivElement>(null);
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null);
  const selectedModel = models.find((model) => model.id === selectedModelId) ?? (selectedModelId ? undefined : models[0]);
  const supportedReasoningEfforts = explicitReasoningEfforts(selectedModel?.supportedReasoningEfforts ?? []);
  const showThinkingSelector = supportedReasoningEfforts.length > 0;
  const selectedThinking = reconcileReasoningEffort(reasoningEffort, supportedReasoningEfforts);
  const imageMismatch = attachments?.items.some((file) => !acceptsAttachment(selectedModel, file));
  const canSend = !preparing && (Boolean(value.trim()) || Boolean(attachments?.items.length)) && !attachments?.busy && !imageMismatch && (Boolean(selectedModel) || requiresSignIn);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  }, [value]);

  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);

  useEffect(() => {
    setOpenMenu(null);
  }, [agentControl?.scope]);

  useEffect(() => {
    if (models.length === 0) {
      setOpenMenu((current) => current === "model" ? null : current);
    }
  }, [models.length]);

  useEffect(() => {
    // Agent owns a top-layer modal, including dismissal and focus handling.
    if (!openMenu || openMenu === "agent") return;
    const closeMenu = (event: MouseEvent) => {
      if (!controlsRef.current?.contains(event.target as Node)) setOpenMenu(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenMenu(null);
    };
    document.addEventListener("mousedown", closeMenu);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("mousedown", closeMenu);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [openMenu]);

  const submit = () => {
    if (!canSend) return;
    onSubmit();
  };

  return (
    <div className="group relative w-full">
      <fieldset disabled={preparing} aria-busy={preparing || undefined} className="composer-shell relative flex min-w-0 flex-col rounded-[22px] pl-4 pr-2.5 pb-2 pt-3 backdrop-blur">
        {attachments ? <><DraftAttachments control={attachments} /><input ref={fileInput} type="file" accept={attachmentAccept(selectedModel)} multiple hidden aria-label="Choose attachments" onChange={(event) => { const files = Array.from(event.target.files ?? []); event.target.value = ""; attachments.onAddFiles(files); }} /></> : null}
        {imageMismatch ? <p role="alert" className="mb-2 text-xs text-[var(--color-warning)]">Choose a model that supports these files, or remove the unsupported attachments.</p> : null}
        <div className="flex items-start gap-3">
          <Lock size={13} aria-hidden="true" className="mt-[5.5px] shrink-0 text-[var(--color-text-muted)]" />
          <textarea
            ref={ref}
            data-chat-input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            onPaste={(event) => { if (attachments && event.clipboardData.files.length) { event.preventDefault(); attachments.onAddFiles(Array.from(event.clipboardData.files)); } }}
            onKeyDown={(event) => {
              // Enter can commit an IME candidate instead of submitting a message.
              // Keep the legacy 229 check for confirmation after compositionend.
              if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                submit();
              }
            }}
            rows={1}
            placeholder={placeholder}
            className="w-full resize-none bg-transparent p-0 text-[14.5px] leading-6 text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)]"
          />
        </div>

        <div className="mt-2 flex items-center justify-between gap-2">
          <div ref={controlsRef} className="flex min-w-0 flex-wrap items-center gap-1.5">
            <div className="relative shrink-0">
              <button
                type="button"
                disabled={models.length === 0 && !requiresSignIn}
                onClick={() => {
                  if (requiresSignIn) {
                    onSignIn?.();
                    return;
                  }
                  setOpenMenu((open) => (open === "model" ? null : "model"));
                }}
                className="flex h-7 items-center gap-1.5 rounded-[7px] bg-[var(--wash-chip)] px-2.5 text-[12px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:hover:bg-[var(--wash-chip)] disabled:hover:text-[var(--color-text-secondary)]"
                aria-haspopup="menu"
                aria-expanded={openMenu === "model"}
                title={requiresSignIn ? "Sign in to choose a model" : models.length === 0 ? modelStatusLabel : "Select model"}
              >
                {selectedModel ? (
                  <>
                    <span title={selectedModel.providerLabel} className="inline-flex shrink-0">
                      <ProviderBrandIcon providerId={selectedModel.providerId} providerLabel={selectedModel.providerLabel} className="pointer-events-none h-3.5 w-3.5" />
                    </span>
                    <ModelBrandIcon model={selectedModel} className="h-3.5 w-3.5 shrink-0" />
                  </>
                ) : (
                  <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-cherry)]" />
                )}
                <span className="max-w-[9rem] truncate sm:max-w-none">
                  {selectedModel?.shortLabel ?? modelStatusLabel}
                </span>
                <ChevronDown size={12} className="opacity-70" />
              </button>
              {openMenu === "model" ? (
                <ModelPicker
                  models={models}
                  selectedModelId={selectedModelId}
                  onSelect={(modelId) => {
                    onModelChange(modelId);
                    setOpenMenu(null);
                  }}
                />
              ) : null}
            </div>

            {showThinkingSelector ? (
              <div className="relative shrink-0">
                <button
                  type="button"
                  onClick={() =>
                    setOpenMenu((open) => (open === "thinking" ? null : "thinking"))
                  }
                  className="flex h-7 items-center gap-1.5 rounded-[7px] bg-[var(--wash-chip)] px-2.5 text-[12px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]"
                  aria-haspopup="menu"
                  aria-expanded={openMenu === "thinking"}
                  title="Thinking level"
                >
                  <BrainCog
                    size={13}
                    className="text-[var(--color-cherry)]"
                  />
                  <span className="hidden sm:inline">
                    {reasoningEffortLabel(selectedThinking)}
                  </span>
                  <ChevronDown size={12} className="opacity-70" />
                </button>
                {openMenu === "thinking" ? (
                  <div
                    role="menu"
                    aria-label="Thinking level"
                    className="glass-panel shadow-glass-pop animate-pop-in absolute bottom-9 left-0 z-30 w-[168px] origin-bottom-left overflow-hidden rounded-[10px] p-1"
                  >
                    {supportedReasoningEfforts.map((effort) => (
                      <button
                        key={effort}
                        type="button"
                        role="menuitemradio"
                        aria-checked={selectedThinking === effort}
                        onClick={() => {
                          onReasoningEffortChange(effort);
                          setOpenMenu(null);
                        }}
                        className="flex w-full items-center gap-2 rounded-[6px] px-2.5 py-2 text-left text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)]"
                      >
                        <span className="flex h-4 w-4 shrink-0 items-center justify-center text-[var(--color-cherry)]">
                          {selectedThinking === effort ? <Check size={13} /> : null}
                        </span>
                        <span className="text-[var(--color-text-primary)]">
                          {reasoningEffortLabel(effort)}
                        </span>
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            ) : null}
            {attachments ? <button type="button" disabled={attachments.busy} aria-label="Attach files" title="Attach images, text/code, or PDFs" className="rounded-md p-1.5 text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)] disabled:opacity-50" onClick={() => fileInput.current?.click()}><Paperclip size={16} /></button> : null}
            <button
              type="button"
              data-web-toggle
              aria-label="Web"
              aria-pressed={webEnabled}
              disabled={isStreaming || webDisabled || !onWebToggle}
              onClick={() => { setOpenMenu(null); onWebToggle?.(); }}
              title={isStreaming ? "Web access is fixed for the current response" : webEnabled ? "Turn off Web" : "Turn on Web"}
              className={`flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] px-2.5 text-[12px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${webEnabled ? "bg-brand text-on-brand enabled:hover:bg-brand-hover dark:bg-[var(--color-cherry)] dark:text-on-accent dark:enabled:hover:bg-[var(--color-cherry)]" : "text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]"}`}
            >
              <Globe2 size={13} aria-hidden="true" />
              Web
              {webEnabled ? <Check size={12} aria-hidden="true" /> : null}
            </button>
            {agentControl ? <AgentModeControl key={agentControl.scope} options={agentControl} open={openMenu === "agent"} onOpenChange={(open) => setOpenMenu(open ? "agent" : null)} /> : null}
          </div>

          <div className="flex shrink-0 items-center gap-2">
            {security !== undefined && onVerifySecurity ? (
              <TeeVerificationBadge key={JSON.stringify([securityScope, security.sessionId, selectedModelId])}
                session={security} model={models.find((model) => model.id === selectedModelId)} modelId={selectedModelId}
                webEnabled={webEnabled} disabledReason={securityRefreshDisabledReason} onVerify={onVerifySecurity} />
            ) : null}
            <ContextUsageIndicator
              usage={contextUsage}
              requestUsage={requestUsage}
              activeTurnId={activeTurnId}
              selectedModelId={selectedModelId}
              reportedModelLabel={models.find((model) => model.id === contextUsage?.modelId)?.label}
              contextWindowTokens={selectedModel?.contextWindowTokens}
              autoCompactThresholdTokens={selectedModel?.autoCompactThresholdTokens}
            />
            {isStreaming ? (
              <button
                type="button"
                onClick={onStop}
                className="flex h-[34px] w-[34px] items-center justify-center rounded-full bg-brand text-on-brand dark:bg-[var(--color-cherry)] dark:text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] hover:bg-brand-hover dark:hover:bg-[var(--color-cherry-bright)] active:scale-[0.94] disabled:cursor-not-allowed disabled:opacity-40"
                aria-label="Stop generating"
              >
                <Square size={12} fill="currentColor" strokeWidth={2.5} />
              </button>
            ) : null}
            {!isStreaming || value.trim() || attachments?.items.length ? (
              <button
                type="button"
                onClick={submit}
                disabled={!canSend}
                className="flex h-[34px] w-[34px] items-center justify-center rounded-full bg-brand text-on-brand dark:bg-[var(--color-cherry)] dark:text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] hover:bg-brand-hover dark:hover:bg-[var(--color-cherry-bright)] active:scale-[0.94] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-brand dark:disabled:hover:bg-[var(--color-cherry)] disabled:active:scale-100"
                aria-label={requiresSignIn ? "Sign in to send" : isStreaming ? "Queue message" : "Send"}
              >
                <ArrowUp size={15} strokeWidth={2.5} />
              </button>
            ) : null}
          </div>
        </div>
      </fieldset>
    </div>
  );
}

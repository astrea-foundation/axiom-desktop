/** @jsxRuntime automatic */
import { useChatLocale } from "./ChatLocale";
import { acceptsAttachment, attachmentAccept } from "@axiom/chat-core/attachments";
import {
  ArrowUp,
  BrainCog,
  Check,
  ChevronDown,
  Globe2,
  Lock,
  LoaderCircle,
  Paperclip,
  Square,
} from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/chat-core/viewTypes";
import type { ProviderModel, ReasoningEffort } from "@axiom/chat-core/types";
import { explicitReasoningEfforts, reconcileReasoningEffort } from "@axiom/chat-core/reasoningSettings";
import { ContextUsageIndicator } from "./ContextUsageIndicator";
import { ModelBrandIcon } from "./ModelBrandIcon";
import { ModelPicker } from "./ModelPicker";
import { COARSE_POINTER_QUERY, useMediaQuery } from "./useMediaQuery";
import { TeeVerificationBadge } from "./TeeVerificationBadge";
import { DraftAttachments, type AttachmentControl } from "./Attachments";
import { AgentModeControl, type AgentControlOptions } from "./AgentModeControl";

let previousComposerBounds: DOMRect | undefined;

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
  isStopping?: boolean;
  models: ProviderModel[];
  /** Models whose provider is temporarily down; shown greyed out in the picker. */
  unavailableModels?: ProviderModel[];
  selectedModelId: string;
  modelStatusLabel: string;
  onModelChange: (modelId: string) => void;
  requiresSignIn?: boolean;
  accountReady?: boolean;
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
  onEnsureSecurity?: () => void | Promise<unknown>;
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
  placeholder,
  autoFocus = false,
  preparing = false,
  isStreaming = false,
  isStopping = false,
  models,
  unavailableModels,
  selectedModelId,
  modelStatusLabel,
  onModelChange,
  requiresSignIn = false,
  accountReady = true,
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
  onEnsureSecurity,
  webEnabled = false,
  webDisabled = false,
  onWebToggle,
}: ChatComposerProps) {
  const { t } = useChatLocale();
  const composerRoot = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = composerRoot.current;
    if (!element) return;
    const current = element.getBoundingClientRect();
    const previous = previousComposerBounds;
    previousComposerBounds = undefined;
    if (previous && !matchMedia("(prefers-reduced-motion: reduce)").matches && Math.abs(previous.y - current.y) > 8) {
      element.animate([{ transform: `translate(${previous.x-current.x}px, ${previous.y-current.y}px)` }, { transform: "translate(0, 0)" }], { duration: 380, easing: "cubic-bezier(.16,1,.3,1)" });
    }
    return () => { previousComposerBounds = element.getBoundingClientRect(); };
  }, []);
  const fileInput = useRef<HTMLInputElement>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const controlsRef = useRef<HTMLDivElement>(null);
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null);
  // Touch keyboards have no Shift+Return, so Return adds a line there.
  const coarsePointer = useMediaQuery(COARSE_POINTER_QUERY);
  const selectedModel = models.find((model) => model.id === selectedModelId) ?? (selectedModelId ? undefined : models[0]);
  const supportedReasoningEfforts = explicitReasoningEfforts(selectedModel?.supportedReasoningEfforts ?? []);
  const showThinkingSelector = supportedReasoningEfforts.length > 0;
  const selectedThinking = reconcileReasoningEffort(reasoningEffort, supportedReasoningEfforts);
  const canAttach = accountReady && !requiresSignIn && !!selectedModel && (selectedModel.supportsImages === true || !!selectedModel.fileMimeTypes?.length);
  const attachmentHint = !selectedModel ? t("composer.attachChooseModel") : !canAttach ? t("composer.attachSupportedModel")
    : t("composer.attachForModel", { model: selectedModel.shortLabel });
  const imageMismatch = attachments?.items.some((file) => !acceptsAttachment(selectedModel, file));
  const canSend = !preparing && (accountReady || requiresSignIn) && (Boolean(value.trim()) || Boolean(attachments?.items.length)) && !attachments?.busy && !imageMismatch && (Boolean(selectedModel) || requiresSignIn);
  const responseActive = isStreaming || isStopping;
  const modelSelectionLocked = preparing || responseActive || (!accountReady && !requiresSignIn);

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
    if (modelSelectionLocked || models.length === 0) {
      setOpenMenu((current) => current === "model" ? null : current);
    }
  }, [modelSelectionLocked, models.length]);

  useEffect(() => {
    // Agent owns its top-layer modal, including dismissal and focus.
    if (!openMenu || openMenu === "agent") return;
    const closeMenu = (event: MouseEvent) => {
      const target = event.target as Element;
      // Phone sheets render outside the controls and close themselves.
      if (!controlsRef.current?.contains(target) && !target.closest?.("[data-composer-popover]")) setOpenMenu(null);
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
    <div ref={composerRoot} className="composer-root group relative w-full">
      <fieldset disabled={preparing} aria-busy={preparing || undefined} className="composer-shell relative flex min-w-0 flex-col rounded-[22px] pl-4 pr-2.5 pb-2 pt-3 backdrop-blur">
        {attachments ? <><DraftAttachments control={attachments} /><input ref={fileInput} type="file" accept={attachmentAccept(selectedModel)} multiple hidden aria-label={t("composer.chooseAttachments")} onChange={(event) => { const files = Array.from(event.target.files ?? []); event.target.value = ""; attachments.onAddFiles(files); }} /></> : null}
        {imageMismatch ? <p role="alert" className="mb-2 text-xs text-[var(--color-warning)]">{t("composer.incompatibleFiles")}</p> : null}
        <div className="composer-layout">
          <div className="composer-input flex items-start gap-3">
            <Lock size={13} aria-hidden="true" className="mt-[5.5px] shrink-0 text-[var(--color-text-muted)]" />
            <textarea
              ref={ref}
              data-chat-input
              value={value}
              onChange={(event) => onChange(event.target.value)}
              onPaste={(event) => { if (attachments && event.clipboardData.files.length) { event.preventDefault(); if (canAttach) attachments.onAddFiles(Array.from(event.clipboardData.files)); } }}
              onKeyDown={(event) => {
                // Enter can commit an IME candidate instead of submitting a message.
                // Keep the legacy 229 check for confirmation after compositionend.
                if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
                if (event.key === "Enter" && !event.shiftKey) {
                  // A touch keyboard's Return adds a line; a hardware keyboard can still send with Cmd/Ctrl+Return.
                  if (coarsePointer && !event.metaKey && !event.ctrlKey) return;
                  event.preventDefault();
                  submit();
                }
              }}
              enterKeyHint={coarsePointer ? "enter" : "send"}
              rows={1}
              placeholder={placeholder ?? t("composer.placeholder")}
              className="w-full resize-none bg-transparent p-0 text-[14.5px] leading-6 text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)]"
            />
          </div>

          <div ref={controlsRef} className="composer-controls flex min-w-0 flex-wrap items-center gap-1.5">
            <div className="composer-model relative shrink-0">
              <button
                type="button"
                disabled={modelSelectionLocked || (models.length === 0 && !requiresSignIn)}
                onClick={() => {
                  if (modelSelectionLocked) return;
                  if (requiresSignIn) {
                    onSignIn?.();
                    return;
                  }
                  setOpenMenu((open) => (open === "model" ? null : "model"));
                }}
                data-model-picker-trigger
                className="composer-model-button flex h-7 items-center gap-1.5 rounded-[7px] bg-[var(--wash-chip)] px-2.5 text-[12px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:opacity-50 disabled:hover:bg-[var(--wash-chip)] disabled:hover:text-[var(--color-text-secondary)]"
                aria-haspopup="menu"
                aria-expanded={openMenu === "model" && !modelSelectionLocked}
                title={!accountReady && !requiresSignIn ? t("composer.accountWaiting") : modelSelectionLocked ? t("composer.modelLocked") : requiresSignIn ? t("composer.signInForModel") : models.length === 0 ? modelStatusLabel : t("composer.selectModel")}
              >
                {selectedModel ? (
                  <ModelBrandIcon model={selectedModel} className="h-3.5 w-3.5 shrink-0" />
                ) : (
                  <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-cherry)]" />
                )}
                <span className="composer-model-label max-w-[9rem] truncate sm:max-w-none">
                  {selectedModel?.shortLabel ?? modelStatusLabel}
                </span>
                <ChevronDown size={12} className="shrink-0 opacity-70" />
              </button>
              {openMenu === "model" && !modelSelectionLocked ? (
                <ModelPicker
                  models={models}
                  unavailableModels={unavailableModels}
                  selectedModelId={selectedModelId}
                  onSelect={(modelId) => {
                    if (modelSelectionLocked) return;
                    onModelChange(modelId);
                    setOpenMenu(null);
                  }}
                  onClose={() => setOpenMenu(null)}
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
                  className="composer-thinking flex h-7 items-center gap-1.5 rounded-[7px] bg-[var(--wash-chip)] px-2.5 text-[12px] font-medium text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]"
                  aria-haspopup="menu"
                  aria-expanded={openMenu === "thinking"}
                  title={t("composer.thinking")}
                >
                  <BrainCog
                    size={13}
                    className="text-[var(--color-cherry)]"
                  />
                  <span className="composer-thinking-label hidden sm:inline">
                    {t(`composer.effort.${selectedThinking}`)}
                  </span>
                  <ChevronDown size={12} className="composer-thinking-chevron opacity-70" />
                </button>
                {openMenu === "thinking" ? (
                  <div
                    role="menu"
                    aria-label={t("composer.thinking")}
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
                          {t(`composer.effort.${effort}`)}
                        </span>
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            ) : null}
            {attachments ? <button type="button" disabled={attachments.busy || !canAttach} aria-label={t("composer.attach")} title={attachmentHint} className="composer-attach rounded-md p-1.5 text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)] disabled:opacity-50" onClick={() => fileInput.current?.click()}><Paperclip size={16} /></button> : null}
            <button
              type="button"
              data-web-toggle
              aria-label={t("composer.web")}
              aria-pressed={webEnabled}
              disabled={responseActive || webDisabled || !onWebToggle}
              onClick={() => { setOpenMenu(null); onWebToggle?.(); }}
              title={responseActive ? t("composer.webLocked") : webEnabled ? t("composer.webOff") : t("composer.webOn")}
              className={`composer-web flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] px-2.5 text-[12px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${webEnabled ? "bg-brand text-on-brand enabled:hover:bg-brand-hover dark:bg-[var(--color-cherry)] dark:text-on-accent dark:enabled:hover:bg-[var(--color-cherry)]" : "text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]"}`}
            >
              <Globe2 size={13} aria-hidden="true" />
              <span className="composer-web-label">{t("composer.web")}</span>
              {webEnabled ? <Check size={12} aria-hidden="true" className="composer-web-check" /> : null}
            </button>
            {agentControl ? <AgentModeControl key={agentControl.scope} options={agentControl} open={openMenu === "agent"} onOpenChange={(open) => setOpenMenu(open ? "agent" : null)} /> : null}
          </div>

          <div className="composer-actions flex shrink-0 items-center gap-2">
            {security !== undefined && onVerifySecurity ? (
              <TeeVerificationBadge key={JSON.stringify([securityScope, security.sessionId, selectedModelId])}
                session={security} model={models.find((model) => model.id === selectedModelId)} modelId={selectedModelId}
                webEnabled={webEnabled} mcpServers={agentControl?.mcpServers} selectedMcpTools={agentControl?.selection.mcpTools} disabledReason={securityRefreshDisabledReason} onVerify={onVerifySecurity} onEnsure={onEnsureSecurity} />
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
          </div>
          <div className="composer-submit flex shrink-0 items-center gap-2">
            {responseActive ? (
              <button
                type="button"
                onClick={onStop}
                disabled={isStopping}
                aria-busy={isStopping || undefined}
                className={`composer-send flex h-[34px] w-[34px] items-center justify-center rounded-full bg-brand text-on-brand dark:bg-[var(--color-cherry)] dark:text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] enabled:hover:bg-brand-hover dark:enabled:hover:bg-[var(--color-cherry-bright)] enabled:active:scale-[0.94] disabled:cursor-wait ${isStopping ? "text-[12px] font-medium sm:w-auto sm:gap-1.5 sm:px-3" : ""}`}
                aria-label={isStopping ? t("composer.stopping") : t("composer.stop")}
              >
                {isStopping ? <><LoaderCircle size={14} aria-hidden="true" className="animate-spin motion-reduce:animate-none" /><span className="hidden sm:inline">{t("composer.stoppingLabel")}</span></> : <Square size={12} aria-hidden="true" fill="currentColor" strokeWidth={2.5} />}
              </button>
            ) : null}
            {!responseActive || value.trim() || attachments?.items.length ? (
              <button
                type="button"
                onClick={submit}
                disabled={!canSend}
                title={!accountReady && !requiresSignIn ? t("composer.accountWaiting") : !selectedModel && !requiresSignIn ? modelStatusLabel : undefined}
                className="composer-send flex h-[34px] w-[34px] items-center justify-center rounded-full bg-brand text-on-brand dark:bg-[var(--color-cherry)] dark:text-on-accent shadow-[0_1px_3px_var(--shade-button)] transition-[background-color,transform] hover:bg-brand-hover dark:hover:bg-[var(--color-cherry-bright)] active:scale-[0.94] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-brand dark:disabled:hover:bg-[var(--color-cherry)] disabled:active:scale-100"
                aria-label={requiresSignIn ? t("composer.signInToSend") : responseActive ? t("composer.queue") : t("composer.send")}
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

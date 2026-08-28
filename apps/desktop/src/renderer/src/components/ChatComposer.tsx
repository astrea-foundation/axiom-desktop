import {
  ArrowUp,
  BrainCog,
  Check,
  ChevronDown,
  Plus,
  Square,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import type { ProviderModel, ReasoningEffort } from "../types";
import { ModelBrandIcon } from "./ModelBrandIcon";
import { ModelPicker } from "./ModelPicker";
import { TeeVerificationBadge } from "./TeeVerificationBadge";

function reasoningEffortLabel(effort: ReasoningEffort): string {
  if (effort === "xhigh") return "Extra high";
  return effort[0]?.toUpperCase() + effort.slice(1);
}

type OpenMenu = "model" | "thinking" | null;

interface ChatComposerProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onStop?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
  isStreaming?: boolean;
  models: ProviderModel[];
  selectedModelId: string;
  modelStatusLabel: string;
  onModelChange: (modelId: string) => void;
  reasoningEffort: ReasoningEffort;
  onReasoningEffortChange: (effort: ReasoningEffort) => void;
  security?: ClientSessionState["security"];
  onVerifySecurity?: () => void;
}

export function ChatComposer({
  value,
  onChange,
  onSubmit,
  onStop,
  placeholder = "Ask anything",
  autoFocus = false,
  isStreaming = false,
  models,
  selectedModelId,
  modelStatusLabel,
  onModelChange,
  reasoningEffort,
  onReasoningEffortChange,
  security,
  onVerifySecurity,
}: ChatComposerProps) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const controlsRef = useRef<HTMLDivElement>(null);
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null);
  const selectedModel = models.find((model) => model.id === selectedModelId) ?? models[0];
  const supportedReasoningEfforts = selectedModel?.supportedReasoningEfforts ?? [];
  const showThinkingSelector = supportedReasoningEfforts.length > 0;
  const canSend = Boolean(value.trim()) && Boolean(selectedModel) && !isStreaming;

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
    if (models.length === 0) {
      setOpenMenu((current) => current === "model" ? null : current);
    }
  }, [models.length]);

  useEffect(() => {
    if (!openMenu) return;
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
      <div className="relative flex flex-col rounded-[26px] border border-white/[0.05] bg-[var(--color-bg-input)]/70 px-3.5 pt-3 pb-2.5 transition-[border-color,box-shadow,background-color] duration-200 hover:border-white/[0.08] hover:bg-[var(--color-bg-input)]/80 focus-within:border-[var(--color-border-accent)] focus-within:bg-[var(--color-bg-input)]/80 focus-within:shadow-[0_0_0_3px_rgba(255,77,109,0.08)]">
        <textarea
          ref={ref}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              submit();
            }
          }}
          rows={1}
          placeholder={placeholder}
          className="w-full resize-none bg-transparent p-0 pl-2 text-[15px] leading-6 text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-tertiary)]"
        />

        <div className="mt-1.5 flex items-center justify-between">
          <div ref={controlsRef} className="flex min-w-0 items-center gap-0.5">
            <button
              type="button"
              className="flex h-8 w-8 items-center justify-center rounded-full text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
              aria-label="Attach"
              title="Attach"
            >
              <Plus size={16} />
            </button>

            <div className="relative shrink-0">
              <button
                type="button"
                disabled={models.length === 0}
                onClick={() => setOpenMenu((open) => (open === "model" ? null : "model"))}
                className="flex items-center gap-1.5 rounded-full px-2.5 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:opacity-70 disabled:hover:bg-transparent disabled:hover:text-[var(--color-text-secondary)]"
                aria-haspopup="menu"
                aria-expanded={openMenu === "model"}
                title={models.length === 0 ? modelStatusLabel : "Select model"}
              >
                {selectedModel ? (
                  <ModelBrandIcon model={selectedModel} className="h-3.5 w-3.5 shrink-0" />
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
                  className="flex items-center gap-1.5 rounded-full px-2.5 py-1.5 text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)]"
                  aria-haspopup="menu"
                  aria-expanded={openMenu === "thinking"}
                  title="Thinking level"
                >
                  <BrainCog
                    size={13}
                    className="text-[var(--color-cherry)]"
                  />
                  <span className="hidden sm:inline">
                    {reasoningEffortLabel(reasoningEffort)}
                  </span>
                  <ChevronDown size={12} className="opacity-70" />
                </button>
                {openMenu === "thinking" ? (
                  <div
                    role="menu"
                    aria-label="Thinking level"
                    className="absolute bottom-9 left-0 z-30 w-[168px] overflow-hidden rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-surface)]/98 p-1 shadow-2xl shadow-black/40 backdrop-blur"
                  >
                    {supportedReasoningEfforts.map((effort) => (
                      <button
                        key={effort}
                        type="button"
                        role="menuitemradio"
                        aria-checked={reasoningEffort === effort}
                        onClick={() => {
                          onReasoningEffortChange(effort);
                          setOpenMenu(null);
                        }}
                        className="flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)]"
                      >
                        <span className="flex h-4 w-4 shrink-0 items-center justify-center text-[var(--color-cherry)]">
                          {reasoningEffort === effort ? <Check size={13} /> : null}
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
          </div>

          <div className="flex items-center gap-1">
            {security !== undefined && onVerifySecurity ? (
              <TeeVerificationBadge security={security} onVerify={onVerifySecurity} />
            ) : null}
            {isStreaming ? (
              <button
                type="button"
                onClick={onStop}
                className="flex h-8 w-8 items-center justify-center rounded-full bg-[var(--color-cherry)] text-[#140709] transition-colors hover:bg-[var(--color-cherry-bright)]"
                aria-label="Stop generating"
              >
                <Square size={12} fill="currentColor" strokeWidth={2.5} />
              </button>
            ) : (
              <button
                type="button"
                onClick={submit}
                disabled={!canSend}
                className="flex h-8 w-8 items-center justify-center rounded-full bg-[var(--color-cherry)] text-[#140709] transition-colors hover:bg-[var(--color-cherry-bright)] disabled:cursor-not-allowed disabled:bg-[var(--color-bg-elevated)] disabled:text-[var(--color-text-tertiary)] disabled:hover:bg-[var(--color-bg-elevated)]"
                aria-label="Send"
              >
                <ArrowUp size={15} strokeWidth={2.5} />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

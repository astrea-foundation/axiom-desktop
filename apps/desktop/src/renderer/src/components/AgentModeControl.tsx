import { Bot, Check } from "lucide-react";
import { useRef } from "react";
import { createPortal } from "react-dom";
import type { ConfigureDesktopAgentRequest } from "@axiom/axiom-acp-client";
import { AgentSettingsDialog } from "./AgentSettingsDialog";

export type AgentSelection = Pick<ConfigureDesktopAgentRequest, "enabled" | "permission" | "workingDirectory">;
export const DEFAULT_AGENT_SELECTION: AgentSelection = { enabled: false, permission: "approve_commands", workingDirectory: null };
export interface AgentControlOptions {
  scope: string;
  selection: AgentSelection;
  defaultDirectory?: string;
  disabledReason?: string;
  onApply(selection: AgentSelection): Promise<void>;
  onChooseDirectory(current?: string): Promise<string | null>;
}

export function AgentModeControl({ options, open, onOpenChange }: {
  options: AgentControlOptions; open: boolean; onOpenChange(open: boolean): void;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  return <>
    <button ref={trigger} type="button" data-agent-toggle aria-label="Agent" aria-haspopup="dialog" aria-expanded={open} aria-pressed={options.selection.enabled}
      onClick={() => onOpenChange(true)}
      className={"flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] px-2.5 text-[12px] font-medium transition-colors " + (options.selection.enabled ? "bg-brand text-on-brand hover:bg-brand-hover dark:bg-[var(--color-cherry)] dark:text-on-accent dark:hover:bg-[var(--color-cherry)]" : "text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]")}>
      <Bot size={14} aria-hidden="true" />Agent{options.selection.enabled ? <Check size={12} aria-hidden="true" /> : null}
    </button>
    {open ? createPortal(<AgentSettingsDialog key={options.scope} options={options} onClose={() => {
      onOpenChange(false);
      trigger.current?.focus({ preventScroll: true });
    }} />, document.body) : null}
  </>;
}

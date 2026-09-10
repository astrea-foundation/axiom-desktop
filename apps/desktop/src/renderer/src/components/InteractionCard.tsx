import { HelpCircle, ShieldAlert } from "lucide-react";
import { useMemo, useState } from "react";
import type { PendingInteraction } from "@axiom/axiom-acp-client";

type Field = { id: string; title: string };

function interactionFields(payload: unknown): Field[] {
  if (!payload || typeof payload !== "object") return [];
  const params = payload as Record<string, unknown>;
  const mode = params.mode && typeof params.mode === "object" ? params.mode as Record<string, unknown> : null;
  const schema = mode?.schema && typeof mode.schema === "object" ? mode.schema as Record<string, unknown> : null;
  const properties = schema?.properties && typeof schema.properties === "object"
    ? schema.properties as Record<string, unknown>
    : {};
  return Object.entries(properties).map(([id, value]) => {
    const property = value && typeof value === "object" ? value as Record<string, unknown> : {};
    return { id, title: typeof property.title === "string" ? property.title : id };
  });
}

export function InteractionCard({ interaction }: { interaction: PendingInteraction }) {
  const fields = useMemo(() => interactionFields(interaction.payload), [interaction.payload]);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const payload = interaction.payload && typeof interaction.payload === "object"
    ? interaction.payload as Record<string, unknown>
    : {};
  const options = Array.isArray(payload.options) ? payload.options as Array<Record<string, unknown>> : [];
  const toolCall = payload.toolCall && typeof payload.toolCall === "object" ? payload.toolCall as Record<string, unknown> : {};

  if (interaction.kind === "permission") {
    return (
      <div className="rounded-xl border border-warning-border bg-warning-soft p-3 text-[12.5px]">
        <div className="mb-2 flex items-center gap-2 text-warning-strong"><ShieldAlert size={14} />Permission requested</div>
        {typeof toolCall.title === "string" ? <p className="mb-3 max-h-60 overflow-y-auto whitespace-pre-wrap break-words text-[var(--color-text-primary)]">{toolCall.title}</p> : null}
        <div className="flex flex-wrap gap-2">
          {options.map((option) => {
            const id = typeof option.optionId === "string" ? option.optionId : "";
            return id ? (
              <button key={id} type="button" onClick={() => void window.axiomDesktop?.agent.resolvePermission(interaction.id, { outcome: "selected", optionId: id })} className="shadow-glass-tile rounded-[8px] bg-[var(--surface-tile)] px-3 py-1.5 text-[var(--color-text-primary)] transition-colors hover:bg-[var(--color-bg-surface-hover)]">
                {typeof option.name === "string" ? option.name : id}
              </button>
            ) : null;
          })}
          {!options.some((option) => option.kind === "reject_once") ? <button type="button" onClick={() => void window.axiomDesktop?.agent.resolvePermission(interaction.id, { outcome: "cancelled" })} className="rounded-lg px-3 py-1.5 text-[var(--color-text-tertiary)]">Deny</button> : null}
        </div>
      </div>
    );
  }

  return (
    <div className="shadow-glass-card rounded-xl bg-[var(--surface-card)] p-3 text-[12.5px]">
      <div className="mb-3 flex items-center gap-2 text-[var(--color-text-primary)]"><HelpCircle size={14} />Axiom needs your input</div>
      <div className="flex flex-col gap-2">
        {fields.map((field) => (
          <label key={field.id} className="flex flex-col gap-1 text-[var(--color-text-secondary)]">
            <span>{field.title}</span>
            <input value={answers[field.id] ?? ""} onChange={(event) => setAnswers((current) => ({ ...current, [field.id]: event.target.value }))} className="rounded-[8px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 py-2 text-[var(--color-text-primary)] outline-none transition-[border-color,box-shadow] hover:border-[var(--border-hover)] focus:border-[var(--color-border-accent)] focus:shadow-[0_0_0_3px_var(--ring-focus-halo)]" />
          </label>
        ))}
        <div className="mt-1 flex gap-2">
          <button type="button" disabled={fields.some((field) => !(answers[field.id] ?? "").trim())} onClick={() => void window.axiomDesktop?.agent.resolveElicitation(interaction.id, { action: "accept", content: answers })} className="rounded-[8px] bg-[var(--color-cherry)] px-3 py-1.5 text-on-accent transition-colors hover:bg-[var(--color-cherry-bright)] disabled:opacity-40">Continue</button>
          <button type="button" onClick={() => void window.axiomDesktop?.agent.resolveElicitation(interaction.id, { action: "decline" })} className="rounded-lg px-3 py-1.5 text-[var(--color-text-tertiary)]">Decline</button>
        </div>
      </div>
    </div>
  );
}

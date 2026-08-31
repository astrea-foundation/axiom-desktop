import { Check, ChevronDown, CircleEllipsis, Globe2, Search, Wrench, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { ToolActivity } from "@axiom/axiom-acp-client";

const MAX_DETAIL_CHARS = 16_000;
const SENSITIVE_KEY = /authorization|api.?key|password|secret|token|cookie/i;

function safeDetail(value: unknown): string {
  const seen = new WeakSet<object>();
  const serialized = JSON.stringify(
    value,
    (key, nested) => {
      if (SENSITIVE_KEY.test(key)) return "[REDACTED]";
      if (nested && typeof nested === "object") {
        if (seen.has(nested)) return "[Circular]";
        seen.add(nested);
      }
      return nested;
    },
    2,
  ) ?? String(value);
  const text = serialized
    .replace(/axm_[A-Za-z0-9_-]+/g, "[REDACTED]")
    .replace(/Bearer\s+[A-Za-z0-9._~-]+/gi, "Bearer [REDACTED]");
  return text.length > MAX_DETAIL_CHARS ? `${text.slice(0, MAX_DETAIL_CHARS)}\n… truncated` : text;
}

function toolIcon(kind: string) {
  if (kind === "search") return <Search size={14} />;
  if (kind === "fetch" || kind === "web") return <Globe2 size={14} />;
  return <Wrench size={14} />;
}

function statusIcon(status: string) {
  if (status === "completed") return <Check size={13} className="text-emerald-400" />;
  if (status === "failed") return <X size={13} className="text-red-400" />;
  return <CircleEllipsis size={13} className="text-[var(--color-cherry)]" />;
}

export function ToolCallCard({ tool }: { tool: ToolActivity }) {
  const [open, setOpen] = useState(false);
  const details = useMemo(
    () => safeDetail({ input: tool.input, output: tool.content, locations: tool.locations }),
    [tool],
  );
  const hasDetails = tool.input !== null || tool.content.length > 0 || tool.locations.length > 0;

  return (
    <div className="ml-11 overflow-hidden rounded-xl border border-[var(--color-border)] bg-[var(--color-bg-surface)]/55 text-[12.5px]">
      <button
        type="button"
        onClick={() => hasDetails && setOpen((value) => !value)}
        className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left text-[var(--color-text-secondary)]"
        aria-expanded={open}
      >
        <span className="text-[var(--color-text-tertiary)]">{toolIcon(tool.kind)}</span>
        <span className="min-w-0 flex-1 truncate">{tool.title}</span>
        <span className="capitalize text-[11px] text-[var(--color-text-tertiary)]">{tool.status.replaceAll("_", " ")}</span>
        {statusIcon(tool.status)}
        {hasDetails ? <ChevronDown size={13} className={`transition-transform ${open ? "" : "-rotate-90"}`} /> : null}
      </button>
      {open ? (
        <pre className="max-h-80 overflow-auto border-t border-[var(--color-border)] bg-black/20 px-3 py-2.5 font-mono text-[11.5px] leading-5 text-[var(--color-text-tertiary)] whitespace-pre-wrap break-words">
          {details}
        </pre>
      ) : null}
    </div>
  );
}

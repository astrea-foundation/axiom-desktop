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
  if (status === "completed") return <Check size={13} className="text-[var(--color-success)]" />;
  if (status === "failed") return <X size={13} className="text-[var(--color-danger)]" />;
  return <CircleEllipsis size={13} className="text-[var(--color-text-tertiary)]" />;
}

export function ToolCallCard({ tool }: { tool: ToolActivity }) {
  const [open, setOpen] = useState(false);
  const details = useMemo(
    () => safeDetail({ input: tool.input, output: tool.content, locations: tool.locations }),
    [tool],
  );
  const hasDetails = tool.input !== null || tool.content.length > 0 || tool.locations.length > 0;

  return (
    <div className="min-w-0 text-[13px]">
      <button
        type="button"
        onClick={() => hasDetails && setOpen((value) => !value)}
        className="inline-flex max-w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
        aria-expanded={open}
      >
        {hasDetails ? (
          <ChevronDown size={14} className={`shrink-0 transition-transform ${open ? "rotate-0" : "-rotate-90"}`} />
        ) : (
          <span className="shrink-0">{toolIcon(tool.kind)}</span>
        )}
        <span className="min-w-0 truncate">{tool.title}</span>
        {statusIcon(tool.status)}
      </button>
      {open ? (
        <pre className="mt-1.5 max-h-80 overflow-auto rounded-[10px] bg-[var(--wash-row)] px-3.5 py-2.5 font-mono text-[11.5px] leading-5 text-[var(--color-text-secondary)] whitespace-pre-wrap break-words">
          {details}
        </pre>
      ) : null}
    </div>
  );
}

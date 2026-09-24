import { ChevronDown, Globe2, LoaderCircle, Search, Wrench, X } from "lucide-react";
import { useId, useMemo, useState } from "react";
import type { ToolActivity } from "@axiom/axiom-acp-client";
import { isFetchTool, redactToolText, toolFailed, toolLabel, toolName, toolSearchSources } from "./toolPresentation";

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
  const text = redactToolText(serialized);
  return text.length > MAX_DETAIL_CHARS ? `${text.slice(0, MAX_DETAIL_CHARS)}\n… truncated` : text;
}

function toolIcon(tool: ToolActivity) {
  if (toolFailed(tool)) return <X size={14} className="text-[var(--color-danger)]" />;
  if (tool.status !== "completed") return <LoaderCircle size={14} className="animate-spin motion-reduce:animate-none" />;
  if (toolName(tool) === "fetch_url" || tool.kind === "fetch" || tool.kind === "web") return <Globe2 size={14} />;
  if (toolName(tool) === "web_search" || tool.kind === "search") return <Search size={14} />;
  return <Wrench size={14} />;
}

export function ToolCallCard({ tool }: { tool: ToolActivity }) {
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const label = toolLabel(tool);
  const sources = useMemo(() => toolSearchSources(tool), [tool]);
  const hasDetails = !isFetchTool(tool) && !toolFailed(tool)
    && (tool.input != null || tool.content.length > 0 || tool.locations.length > 0);
  const details = useMemo(
    () => hasDetails && !sources ? safeDetail({ input: tool.input, output: tool.content, locations: tool.locations }) : "",
    [tool, hasDetails, sources],
  );
  const rowClass = "flex min-h-8 max-w-full items-center gap-2 rounded-md px-1.5 py-1 text-left leading-5 text-[var(--color-text-secondary)]";
  const row = <>
    <span className="shrink-0 text-[var(--color-text-tertiary)]" aria-hidden="true">{toolIcon(tool)}</span>
    <span className="min-w-0 truncate">{label}</span>
    {sources ? <span className="shrink-0 text-[11px] text-[var(--color-text-tertiary)]">{sources.length} {sources.length === 1 ? "source" : "sources"}</span> : null}
    {hasDetails ? <ChevronDown aria-hidden="true" size={12} className={`shrink-0 text-[var(--color-text-tertiary)] transition-transform ${open ? "rotate-0" : "-rotate-90"}`} /> : null}
  </>;

  return (
    <div className="min-w-0 text-[13px]" data-tool-call-id={tool.callId}>
      {hasDetails ? <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className={`${rowClass} transition-colors hover:bg-[var(--color-bg-surface-hover)]`}
        aria-expanded={open}
        aria-controls={detailsId}
        aria-label={`${label}; show details`}
        title={label}
      >
        {row}
      </button> : <div className={rowClass} title={label}>{row}</div>}
      {hasDetails && open ? (
        <div id={detailsId} className="ml-3.5 mt-1 max-h-80 overflow-auto border-l border-[var(--color-border)] pl-4 pr-2 text-[12px] leading-5 text-[var(--color-text-secondary)]">
          {sources ? (
            sources.length ? <ul className="space-y-3 py-1">
              {sources.map((source, index) => <li key={`${index}:${source.url}`} className="min-w-0 break-words">
                <a href={source.url} target="_blank" rel="noopener noreferrer" className="font-medium text-[var(--color-text-primary)] hover:underline">{source.title}</a>
                <div className="truncate text-[11px] text-[var(--color-text-tertiary)]">{source.host}</div>
                {source.snippet ? <p>{source.snippet}</p> : null}
              </li>)}
            </ul> : <p className="py-1">No results found.</p>
          ) : <pre className="py-1 font-mono text-[11.5px] whitespace-pre-wrap break-words">{details}</pre>}
        </div>
      ) : null}
    </div>
  );
}

import { useEffect, useId, useRef, useState } from "react";
import type { ClientSessionState } from "@axiom/axiom-acp-client";

interface ContextUsageIndicatorProps {
  usage: ClientSessionState["contextUsage"];
  requestUsage?: ClientSessionState["requestUsage"];
  activeTurnId?: string | null;
  contextWindowTokens?: number;
  autoCompactThresholdTokens?: number;
  selectedModelId?: string;
  reportedModelLabel?: string;
}

const tokens = new Intl.NumberFormat("en-US");
const percent = new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 });
const validCount = (value: number | undefined): value is number =>
  value !== undefined && Number.isSafeInteger(value) && value >= 0;

export function contextUsagePresentation({ usage, contextWindowTokens, autoCompactThresholdTokens }: ContextUsageIndicatorProps) {
  // A saved request must never be divided by a subsequently selected model's
  // capacity. Reports without their own limits remain explicitly unavailable.
  const window = usage ? usage.contextWindowTokens ?? undefined : contextWindowTokens;
  const compactAt = usage ? usage.autoCompactThresholdTokens ?? undefined : autoCompactThresholdTokens;
  const capacity = validCount(window) && window > 0 ? window : null;
  const used = usage && validCount(usage.inputTokens) && validCount(usage.outputTokens)
    && Number.isSafeInteger(usage.inputTokens + usage.outputTokens) ? usage.inputTokens + usage.outputTokens : null;
  const percentage = capacity !== null && used !== null ? used / capacity * 100 : null;
  const threshold = capacity !== null && validCount(compactAt)
    && compactAt > 0 && compactAt <= capacity ? compactAt : null;
  const usageLabel = used === null ? "No usage reported yet" : `${tokens.format(used)} tokens`;
  const breakdownLabel = used !== null && usage ? `${tokens.format(usage.inputTokens)} input, ${tokens.format(usage.outputTokens)} output` : null;
  const capacityLabel = capacity === null ? "Context size unavailable" : `${tokens.format(capacity)} token context window`;
  const percentageLabel = percentage === null ? "—" : `${percent.format(percentage)}% used`;
  const compactionLabel = threshold === null ? "Compaction threshold unavailable"
    : `Auto-compacts at ${tokens.format(threshold)} tokens (${percent.format(threshold / capacity! * 100)}%)`;
  return { percentage, usageLabel, breakdownLabel, capacityLabel, percentageLabel, compactionLabel };
}

type RequestUsage = NonNullable<ClientSessionState["requestUsage"]>[number];
const money = (value: bigint) => `$${value / 1000000n}.${(value % 1000000n).toString().padStart(6, "0")}`;

export function requestUsageTotals(records: RequestUsage[]) {
  // Snapshots replace cumulative counters; duplicate deliveries are not charges.
  const unique = [...new Map(records.map((record) => [record.requestId, record])).values()];
  const sum = (key: "inputTokens" | "outputTokens" | "costMicrousd" | "cachedInputTokens" | "reasoningTokens") =>
    unique.reduce((total, request) => total + BigInt(request[key] ?? "0"), 0n);
  const starts = unique.map((r) => Number(r.startedAtMs)).filter(Number.isFinite);
  const ends = unique.map((r) => r.state === "running" ? Date.now() : r.finishedAtMs == null ? NaN : Number(r.finishedAtMs));
  const elapsed = starts.length === unique.length && ends.every(Number.isFinite) && unique.length
    ? Math.max(0, Math.max(...ends) - Math.min(...starts)) : null;
  return { records: unique, sum, elapsed, cost: sum("costMicrousd"),
    unknown: unique.some((r) => r.inputTokens == null || r.outputTokens == null),
    pending: unique.some((r) => !r.settled || r.costMicrousd == null) };
}

export function ContextUsageIndicator(props: ContextUsageIndicatorProps) {
  const panelId = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return;
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [open]);
  const requests = requestUsageTotals(props.requestUsage ?? []).records;
  const conversation = requests.filter((request) => request.purpose === "conversation");
  const latest = conversation.at(-1);
  const latestKnown = latest && latest.inputTokens != null && latest.outputTokens != null;
  const usage = latestKnown ? {
    inputTokens: Number(latest.inputTokens), outputTokens: Number(latest.outputTokens),
    modelId: latest.modelId, reportedAt: latest.finishedAtMs ?? latest.startedAtMs ?? "",
    contextWindowTokens: latest.contextWindowTokens ?? (latest.modelId === props.usage?.modelId ? props.usage.contextWindowTokens : undefined),
    autoCompactThresholdTokens: latest.autoCompactThresholdTokens ?? (latest.modelId === props.usage?.modelId ? props.usage.autoCompactThresholdTokens : undefined),
  } : props.usage;
  const view = contextUsagePresentation({ ...props, usage });
  const turnId = props.activeTurnId ?? latest?.turnId;
  const turn = conversation.filter((request) => request.turnId === turnId && turnId != null);
  const { sum, unknown, pending, cost, elapsed } = requestUsageTotals(turn);
  const overhead = (["title", "compaction"] as const).map((purpose) => ({ purpose, ...requestUsageTotals(requests.filter((r) => r.purpose === purpose)) }));
  const contextState = latest?.state === "running" ? "Previous request (reply in progress)"
    : latest?.completeness === "partial" ? "Partial request usage"
    : latest?.completeness === "unknown" ? "Latest usage unavailable; showing previous request" : null;
  const fill = Math.min(100, Math.max(0, view.percentage ?? 0));
  return (
    <div
      ref={root}
      className="relative shrink-0"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={(event) => { if (!event.currentTarget.contains(document.activeElement)) setOpen(false); }}
      onFocus={() => setOpen(true)}
      onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false); }}
      onKeyDown={(event) => { if (event.key === "Escape") { trigger.current?.focus(); setOpen(false); event.stopPropagation(); } }}
    >
      <button
        ref={trigger}
        type="button"
        aria-label="Usage"
        aria-describedby={`${panelId}-summary`}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen(true)}
        className="relative flex h-8 w-8 cursor-default items-center justify-center rounded-full text-[var(--color-text-tertiary)] outline-none transition-colors hover:bg-[var(--wash-chip-hover)] focus-visible:ring-2 focus-visible:ring-[var(--color-text-muted)]"
      >
        <span data-context-usage role={view.percentage === null ? "img" : "meter"} aria-label="Usage"
          aria-valuemin={view.percentage === null ? undefined : 0}
          aria-valuemax={view.percentage === null ? undefined : 100}
          aria-valuenow={view.percentage === null ? undefined : fill}
          aria-valuetext={`Usage: ${view.percentageLabel}. ${view.usageLabel}. ${view.capacityLabel}. ${view.compactionLabel}.`}
          className="flex h-full w-full items-center justify-center">
          <svg aria-hidden="true" className="absolute h-7 w-7 -rotate-90" viewBox="0 0 32 32" fill="none">
            <circle cx="16" cy="16" r="13" stroke="currentColor" strokeWidth="2.5" opacity="0.2" />
            <circle cx="16" cy="16" r="13" strokeWidth="2.5" stroke="currentColor" pathLength="100" strokeDasharray={`${fill} 100`} strokeLinecap={fill > 0 ? "round" : "butt"} />
          </svg>
          <span aria-hidden="true" className="text-[9px] font-medium tabular-nums">
            {view.percentage === null ? "—" : Math.floor(view.percentage)}
          </span>
        </span>
      </button>
      <span id={`${panelId}-summary`} className="sr-only">{view.percentageLabel}. {view.usageLabel}. {view.capacityLabel}. {view.compactionLabel}.</span>
      {open ? (
        <div className="absolute bottom-full right-0 z-50 w-[276px] max-w-[calc(100vw-48px)] pb-2">
          <div id={panelId} role="dialog" aria-label="Usage" className="max-h-[65vh] overflow-y-auto rounded-xl bg-[var(--surface-float)] p-3 text-[12px] leading-5 text-[var(--color-text-secondary)] shadow-glass-pop backdrop-blur-xl">
            <div className="mb-1 flex items-center justify-between gap-3 font-medium text-[var(--color-text-primary)]">
              <span>Usage</span><span>{view.percentageLabel}</span>
            </div>
            {contextState ? <div className="text-[11px] text-[var(--color-text-tertiary)]">{contextState}</div> : null}
            <div>{view.usageLabel}</div>
            <div>{view.capacityLabel}</div>
            {usage && usage.modelId !== props.selectedModelId ? (
              <div className="mt-1 break-words text-[11px]">Reported for {usage.modelId === props.usage?.modelId ? props.reportedModelLabel ?? usage.modelId : usage.modelId}</div>
            ) : null}
            <div className="mt-2 border-t border-[var(--ring-hairline)] pt-2">{view.compactionLabel}</div>
            {view.breakdownLabel || turn.length || overhead.some((item) => item.records.length) ? <details className="mt-2 border-t border-[var(--ring-hairline)] pt-2 text-[11px]">
              <summary className="cursor-pointer font-medium text-[var(--color-text-primary)]">Usage details</summary>
              {view.breakdownLabel ? <div className="mt-2"><div className="font-medium">Last request</div><div>{view.breakdownLabel}</div></div> : null}
              {turn.length ? <div className="mt-2">
                <div className="font-medium text-[var(--color-text-primary)]">This turn ({turn.length} model {turn.length === 1 ? "request" : "requests"})</div>
                <div>{unknown ? "At least " : ""}{tokens.format(sum("inputTokens"))} input, {tokens.format(sum("outputTokens"))} output</div>
                {sum("cachedInputTokens") > 0n ? <div>{tokens.format(sum("cachedInputTokens"))} cached input (included above)</div> : null}
                {sum("reasoningTokens") > 0n ? <div>{tokens.format(sum("reasoningTokens"))} reasoning output (included above)</div> : null}
                <div>{money(cost)} {pending ? "reported (settlement pending)" : "settled"}</div>
                {elapsed != null ? <div>{Math.round(elapsed / 1000)} seconds elapsed</div> : null}
                {unknown ? <div>Some token counts are unavailable.</div> : null}
                {turn.some((request) => request.errorCode === "CANCELLED_USAGE_WAIVED") ? <div>Cancelled requests without final usage weren’t charged.</div> : null}
              </div> : null}
              {overhead.some((item) => item.records.length) ? <div className="mt-2"><div className="font-medium">Other thread usage</div>
                {overhead.filter((item) => item.records.length).map((item) => <div key={item.purpose}>{item.purpose === "title" ? "Titles" : "Compaction"}: {money(item.cost)}{item.pending ? " reported (settlement pending)" : " settled"}</div>)}
              </div> : null}
            </details> : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}

import { ChartPie, LogIn, RefreshCw } from "lucide-react";
import { useEffect, useId, useState } from "react";
import type { UsageSummary } from "@axiom/axiom-acp-client";
import { formatSpend, pieSlice, shareLabel, spendingBreakdown, type UsagePeriod } from "../usageSpend";

const COLORS = ["#ff7653", "#9295c9", "#73a79c", "#d7ad68", "#77a5c6", "#c58eaa", "#ada687", "#7e8fa6"];
const buttonClass = "inline-flex items-center gap-2 rounded-lg border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-2 text-[12.5px] text-[var(--color-text-secondary)] hover:bg-[var(--surface-button-secondary-hover)] disabled:opacity-50";

const PERIODS: { id: UsagePeriod; label: string; totalLabel: string }[] = [
  { id: "week", label: "This week", totalLabel: "Spending this week" },
  { id: "month", label: "This month", totalLabel: "Spending this month" },
  { id: "all_time", label: "All time", totalLabel: "All-time spending" },
];
type UsageState = { requestKey: string } & ({ kind: "loading" } | { kind: "error" } | { kind: "ready"; summary: UsageSummary });

export function UsagePanel({ accountId, connected, onLogin }: {
  accountId: string | null;
  connected: boolean;
  onLogin: () => void;
}) {
  const [state, setState] = useState<UsageState>({ kind: "loading", requestKey: "" });
  const [refresh, setRefresh] = useState(0);
  const [period, setPeriod] = useState<UsagePeriod>("all_time");
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const requestKey = JSON.stringify([accountId, connected, period, timezone, refresh]);
  const panelId = useId();
  useEffect(() => {
    if (!accountId || !connected) return;
    let cancelled = false;
    setState({ kind: "loading", requestKey });
    void (async () => {
      try {
        const api = window.axiomDesktop?.agent;
        if (!api?.usageSummary) throw new Error("Usage is unavailable");
        const { summary } = await api.usageSummary(accountId, { period, timezone });
        spendingBreakdown(summary, period);
        if (!cancelled) setState({ kind: "ready", summary, requestKey });
      } catch {
        if (!cancelled) setState({ kind: "error", requestKey });
      }
    })();
    return () => { cancelled = true; };
  }, [accountId, connected, period, timezone, requestKey]);

  if (!accountId) return <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
    <p className="mb-4 text-[13px] text-[var(--color-text-secondary)]">Sign in to see your spending.</p>
    <button type="button" onClick={onLogin} disabled={!connected} className={buttonClass}><LogIn size={14} />Sign in</button>
  </div>;
  if (!connected) return <p role="status" className="text-[13px] text-[var(--color-text-secondary)]">Reconnect to load your usage.</p>;
  const current = state.requestKey === requestKey ? state : { kind: "loading" as const };
  return <div data-usage-panel>
    <div role="tablist" aria-label="Spending period" className="mb-6 inline-flex max-w-full gap-1 rounded-[11px] border border-[var(--color-border)] bg-[var(--wash-row)] p-1">
      {PERIODS.map((item, index) => <button key={item.id} id={`${panelId}-${item.id}`} type="button" role="tab"
        aria-selected={period === item.id} aria-controls={`${panelId}-content`} tabIndex={period === item.id ? 0 : -1}
        onClick={() => setPeriod(item.id)}
        onKeyDown={event => {
          const next = event.key === "ArrowRight" ? (index + 1) % PERIODS.length
            : event.key === "ArrowLeft" ? (index + PERIODS.length - 1) % PERIODS.length
            : event.key === "Home" ? 0 : event.key === "End" ? PERIODS.length - 1 : null;
          if (next === null) return;
          event.preventDefault();
          setPeriod(PERIODS[next]!.id);
          document.getElementById(`${panelId}-${PERIODS[next]!.id}`)?.focus();
        }}
        className={`rounded-lg px-3 py-1.5 text-[12.5px] transition-colors motion-reduce:transition-none ${period === item.id ? "bg-[var(--surface-button-secondary)] text-[var(--color-text-primary)] shadow-sm" : "text-[var(--color-text-tertiary)] hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-secondary)]"}`}>
        {item.label}
      </button>)}
    </div>
    <div role="tabpanel" tabIndex={0} id={`${panelId}-content`} aria-labelledby={`${panelId}-${period}`} aria-busy={current.kind === "loading"}>
      {current.kind === "loading" ? <div role="status" className="flex items-center gap-2 py-8 text-[13px] text-[var(--color-text-tertiary)]"><RefreshCw size={15} className="animate-spin motion-reduce:animate-none" />Loading usage…</div>
        : current.kind === "error" ? <div role="alert" className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
          <p className="mb-4 text-[13px] text-[var(--color-text-secondary)]">Couldn’t load your usage.</p>
          <button type="button" onClick={() => setRefresh(value => value + 1)} className={buttonClass}><RefreshCw size={14} />Try again</button>
        </div>
        : <UsageBreakdown key={requestKey} summary={current.summary} period={period} onRefresh={() => setRefresh(value => value + 1)} />}
    </div>
    <p className="mt-4 text-[11px] text-[var(--color-text-tertiary)]">Weeks start Monday. Dates use your local timezone ({timezone.replaceAll("_", " ")}).</p>
  </div>;
}

function UsageBreakdown({ summary, period, onRefresh }: { summary: UsageSummary; period: UsagePeriod; onRefresh: () => void }) {
  const [activeModel, setActiveModel] = useState<string | null>(null);
  const chartId = useId();
  const { total, models } = spendingBreakdown(summary, period);
  let start = 0;
  let cumulative = 0n;
  const slices = models.map((model, index) => {
    cumulative += model.amount;
    const end = index === models.length - 1 ? 1 : Number(cumulative * 1_000_000_000n / total) / 1_000_000_000;
    const path = pieSlice(start, end);
    start = end;
    return { ...model, path, color: COLORS[index % COLORS.length]! };
  });

  return <div>
    <div className="flex items-start justify-between gap-4">
      <div>
        <p className="text-[12px] text-[var(--color-text-tertiary)]">{PERIODS.find(item => item.id === period)!.totalLabel}</p>
        <p data-usage-total className="mt-2 break-all text-[36px] font-medium leading-tight tracking-[-0.04em] text-[var(--color-text-primary)] tabular-nums">{formatSpend(total)}</p>
      </div>
      <button type="button" onClick={onRefresh} aria-label="Refresh usage" className={`${buttonClass} !p-2`}><RefreshCw size={15} /></button>
    </div>
    {models.length === 0 ? <div className="mt-7 flex flex-col items-center rounded-[16px] border border-[var(--color-border)] bg-[var(--wash-row)] px-5 py-12 text-center">
      <ChartPie size={32} strokeWidth={1.2} className="mb-4 text-[var(--color-text-muted)]" />
      <h3 className="text-[15px] font-medium text-[var(--color-text-primary)]">{period === "all_time" ? "No spending yet" : `No spending this ${period}`}</h3>
      <p className="mt-1 text-[12.5px] text-[var(--color-text-tertiary)]">{period === "all_time" ? "Your model breakdown will appear here after your first charge." : "Charges for this period will appear here."}</p>
    </div> : <div className="mt-7 rounded-[16px] border border-[var(--color-border)] bg-[var(--wash-row)] p-4 sm:p-5">
      <h3 className="text-[13px] font-medium text-[var(--color-text-primary)]">Spending by model</h3>
      <div className="mt-3 flex flex-wrap items-center justify-center gap-4">
        <svg viewBox="0 0 240 240" role="img" aria-labelledby={`${chartId}-title ${chartId}-desc`} className="w-[220px] max-w-full shrink-0">
          <title id={`${chartId}-title`}>Spending by model</title>
          <desc id={`${chartId}-desc`}>{models.map(model => `${model.label}: ${formatSpend(model.amount)}, ${shareLabel(model)}`).join(". ")}</desc>
          {slices.map(model => models.length === 1
            ? <circle key={model.id} cx="120" cy="120" r="106" fill={model.color} />
            : <path key={model.id} d={model.path} fill={model.color} stroke="var(--surface-card)" strokeWidth="2"
                opacity={activeModel && activeModel !== model.id ? 0.35 : 1}
                className="transition-opacity motion-reduce:transition-none" onMouseEnter={() => setActiveModel(model.id)} onMouseLeave={() => setActiveModel(null)} />)}
        </svg>
        <ul aria-label="Model spending" className="min-w-0 flex-1 basis-[240px] space-y-1">
          {slices.map(model => <li key={model.id}>
            <button type="button" className={`flex w-full items-center gap-3 rounded-[9px] p-2.5 text-left transition-colors ${activeModel === model.id ? "bg-[var(--wash-chip)]" : "hover:bg-[var(--wash-hover)]"}`}
              onMouseEnter={() => setActiveModel(model.id)} onMouseLeave={() => setActiveModel(null)} onFocus={() => setActiveModel(model.id)} onBlur={() => setActiveModel(null)} onClick={() => setActiveModel(model.id)}
              aria-label={`${model.label}, ${model.provider}, ${formatSpend(model.amount)}, ${shareLabel(model)} of spending`}>
              <span aria-hidden="true" className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ backgroundColor: model.color }} />
              <span className="min-w-0 flex-1"><span className="block break-words text-[12.5px] font-medium text-[var(--color-text-primary)]">{model.label}</span>
                <span className="block text-[11px] text-[var(--color-text-tertiary)]">{model.provider}</span></span>
              <span className="shrink-0 text-right tabular-nums"><span className="block text-[12.5px] text-[var(--color-text-primary)]">{formatSpend(model.amount)}</span><span className="block text-[11px] text-[var(--color-text-tertiary)]">{shareLabel(model)}</span></span>
            </button>
          </li>)}
        </ul>
      </div>
    </div>}
  </div>;
}

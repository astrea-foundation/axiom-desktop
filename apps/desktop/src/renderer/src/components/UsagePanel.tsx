import { ChartPie, LogIn, RefreshCw } from "lucide-react";
import { useEffect, useId, useState } from "react";
import type { UsageSummary } from "@axiom/axiom-acp-client";
import { formatSpend, pieSlice, shareLabel, spendingBreakdown } from "../usageSpend";

const COLORS = ["#ff7653", "#9295c9", "#73a79c", "#d7ad68", "#77a5c6", "#c58eaa", "#ada687", "#7e8fa6"];
const buttonClass = "inline-flex items-center gap-2 rounded-lg border border-[var(--color-border)] bg-[var(--surface-button-secondary)] px-3 py-2 text-[12.5px] text-[var(--color-text-secondary)] hover:bg-[var(--surface-button-secondary-hover)] disabled:opacity-50";

type UsageState = { kind: "loading" } | { kind: "error" } | { kind: "ready"; summary: UsageSummary };

export function UsagePanel({ accountId, connected, onLogin }: {
  accountId: string | null;
  connected: boolean;
  onLogin: () => void;
}) {
  const [state, setState] = useState<UsageState>({ kind: "loading" });
  const [refresh, setRefresh] = useState(0);
  const [activeModel, setActiveModel] = useState<string | null>(null);
  const chartId = useId();
  useEffect(() => {
    if (!accountId || !connected) return;
    let cancelled = false;
    setState({ kind: "loading" });
    setActiveModel(null);
    void (async () => {
      try {
        const api = window.axiomDesktop?.agent;
        if (!api?.usageSummary) throw new Error("Usage is unavailable");
        const { summary } = await api.usageSummary(accountId);
        spendingBreakdown(summary);
        if (!cancelled) setState({ kind: "ready", summary });
      } catch {
        if (!cancelled) setState({ kind: "error" });
      }
    })();
    return () => { cancelled = true; };
  }, [accountId, connected, refresh]);

  if (!accountId) return <div className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
    <p className="mb-4 text-[13px] text-[var(--color-text-secondary)]">Sign in to see your spending.</p>
    <button type="button" onClick={onLogin} disabled={!connected} className={buttonClass}><LogIn size={14} />Sign in</button>
  </div>;
  if (!connected) return <p role="status" className="text-[13px] text-[var(--color-text-secondary)]">Reconnect to load your usage.</p>;
  if (state.kind === "loading") return <div role="status" className="flex items-center gap-2 py-8 text-[13px] text-[var(--color-text-tertiary)]"><RefreshCw size={15} className="animate-spin motion-reduce:animate-none" />Loading usage…</div>;
  if (state.kind === "error") return <div role="alert" className="rounded-[13px] border border-[var(--color-border)] bg-[var(--wash-row)] p-5">
    <p className="mb-4 text-[13px] text-[var(--color-text-secondary)]">Couldn’t load your usage.</p>
    <button type="button" onClick={() => setRefresh(value => value + 1)} className={buttonClass}><RefreshCw size={14} />Try again</button>
  </div>;

  const { total, models } = spendingBreakdown(state.summary);
  let start = 0;
  let cumulative = 0n;
  const slices = models.map((model, index) => {
    cumulative += model.amount;
    const end = index === models.length - 1 ? 1 : Number(cumulative * 1_000_000_000n / total) / 1_000_000_000;
    const path = pieSlice(start, end);
    start = end;
    return { ...model, path, color: COLORS[index % COLORS.length]! };
  });

  return <div data-usage-panel>
    <div className="flex items-start justify-between gap-4">
      <div>
        <p className="text-[12px] text-[var(--color-text-tertiary)]">Total spent <span aria-hidden="true">·</span> All time</p>
        <p data-usage-total className="mt-2 break-all text-[36px] font-medium leading-tight tracking-[-0.04em] text-[var(--color-text-primary)] tabular-nums">{formatSpend(total)}</p>
      </div>
      <button type="button" onClick={() => setRefresh(value => value + 1)} aria-label="Refresh usage" className={`${buttonClass} !p-2`}><RefreshCw size={15} /></button>
    </div>
    {models.length === 0 ? <div className="mt-7 flex flex-col items-center rounded-[16px] border border-[var(--color-border)] bg-[var(--wash-row)] px-5 py-12 text-center">
      <ChartPie size={32} strokeWidth={1.2} className="mb-4 text-[var(--color-text-muted)]" />
      <h3 className="text-[15px] font-medium text-[var(--color-text-primary)]">No spending yet</h3>
      <p className="mt-1 text-[12.5px] text-[var(--color-text-tertiary)]">Your model breakdown will appear here after your first charge.</p>
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
    <p className="mt-4 text-[12px] leading-5 text-[var(--color-text-tertiary)]">Posted inference charges, including credit used.</p>
  </div>;
}

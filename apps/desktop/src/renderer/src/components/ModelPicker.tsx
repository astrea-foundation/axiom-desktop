import { Check, Search } from "lucide-react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ProviderModel } from "../types";
import { ModelBrandIcon, ProviderBrandIcon } from "./ModelBrandIcon";

interface ProviderGroup {
  id: string;
  label: string;
  models: ProviderModel[];
}

interface ModelPickerProps {
  models: ProviderModel[];
  selectedModelId: string;
  onSelect: (modelId: string) => void;
}

function groupModels(models: ProviderModel[]): ProviderGroup[] {
  const groups = new Map<string, ProviderGroup>();
  for (const model of models) {
    const existing = groups.get(model.providerId);
    if (existing) {
      existing.models.push(model);
    } else {
      groups.set(model.providerId, {
        id: model.providerId,
        label: model.providerLabel,
        models: [model],
      });
    }
  }
  return [...groups.values()];
}

function matches(model: ProviderModel, query: string): boolean {
  if (!query) return true;
  const haystack = [
    model.label,
    model.shortLabel,
    model.id,
    model.model,
    model.providerLabel,
  ].join(" ").toLocaleLowerCase();
  return haystack.includes(query);
}

export function ModelPicker({ models, selectedModelId, onSelect }: ModelPickerProps) {
  const pickerRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const downwardOffsetRef = useRef(0);
  const [downwardOffset, setDownwardOffset] = useState(0);
  const groups = useMemo(() => groupModels(models), [models]);
  const selectedProviderId = models.find((model) => model.id === selectedModelId)?.providerId;
  const [activeProviderId, setActiveProviderId] = useState(
    selectedProviderId ?? groups[0]?.id ?? "",
  );
  const [search, setSearch] = useState("");
  const normalizedSearch = search.trim().toLocaleLowerCase();

  const filteredGroups = useMemo(
    () => groups.map((group) => ({
      ...group,
      models: group.models.filter((model) => matches(model, normalizedSearch)),
    })),
    [groups, normalizedSearch],
  );
  const activeGroup = filteredGroups.find((group) => group.id === activeProviderId)
    ?? filteredGroups[0];

  useLayoutEffect(() => {
    const picker = pickerRef.current;
    if (!picker) return;
    const safeTop = 52;
    let frame = 0;
    const measure = () => {
      const parentTop = picker.offsetParent?.getBoundingClientRect().top ?? 0;
      const naturalTop = parentTop + picker.offsetTop - downwardOffsetRef.current;
      const nextOffset = Math.max(0, Math.ceil(safeTop - naturalTop));
      if (nextOffset === downwardOffsetRef.current) return;
      downwardOffsetRef.current = nextOffset;
      setDownwardOffset(nextOffset);
    };
    const queueMeasurement = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(measure);
    };
    const observer = new ResizeObserver(queueMeasurement);
    observer.observe(picker);
    window.addEventListener("resize", queueMeasurement);
    measure();
    return () => {
      window.cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", queueMeasurement);
    };
  }, []);

  return (
    <div
      ref={pickerRef}
      role="dialog"
      aria-label="Choose a model"
      style={{ bottom: `calc(2.5rem - ${downwardOffset}px)` }}
      className="animate-pop-in absolute left-0 z-40 flex max-h-[min(430px,calc(100vh-4rem))] w-[min(32rem,calc(100vw-3rem))] origin-bottom-left flex-col overflow-hidden rounded-2xl border border-white/[0.08] bg-[var(--color-bg-surface)]/98 shadow-2xl shadow-black/50 backdrop-blur-xl"
    >
      <div className="border-b border-white/[0.06] px-3.5 pb-0 pt-3.5">
        <div className="mb-3 text-center text-[13px] font-medium text-[var(--color-text-primary)]">
          Choose your model
        </div>

        <label className="flex h-9 items-center gap-2 rounded-lg border border-white/[0.06] bg-[var(--color-bg-input)] px-2.5 transition-colors focus-within:border-[var(--color-border-accent)]">
          <Search size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
          <input
            ref={searchRef}
            autoFocus
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search models"
            className="min-w-0 flex-1 bg-transparent text-[12.5px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-tertiary)]"
          />
        </label>

        <div
          role="tablist"
          aria-label="Model providers"
          className="mt-2.5 flex gap-1 overflow-x-auto pb-2"
        >
          {filteredGroups.map((group) => {
            const active = group.id === activeGroup?.id;
            return (
              <button
                key={group.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => {
                  setActiveProviderId(group.id);
                  searchRef.current?.focus();
                }}
                className={[
                  "flex shrink-0 items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] transition-colors",
                  active
                    ? "bg-[var(--color-cherry-glow)] text-[var(--color-text-primary)] ring-1 ring-inset ring-[var(--color-border-accent)]"
                    : "text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]",
                ].join(" ")}
              >
                <ProviderBrandIcon
                  providerId={group.id}
                  providerLabel={group.label}
                  className="h-3.5 w-3.5 shrink-0"
                />
                <span>{group.label}</span>
                <span
                  className={[
                    "min-w-5 rounded-full px-1.5 py-0.5 text-center text-[10px] tabular-nums",
                    active
                      ? "bg-[var(--color-cherry)]/15 text-[var(--color-cherry-bright)]"
                      : "bg-white/[0.05] text-[var(--color-text-muted)]",
                  ].join(" ")}
                >
                  {group.models.length}
                </span>
              </button>
            );
          })}
        </div>
      </div>

      <div role="tabpanel" className="min-h-0 overflow-y-auto p-2">
        {activeGroup?.models.length ? (
          activeGroup.models.map((model) => {
            const selected = model.id === selectedModelId;
            return (
              <button
                key={model.id}
                type="button"
                onClick={() => onSelect(model.id)}
                className={[
                  "group/model flex w-full items-center gap-3 rounded-xl px-2.5 py-2.5 text-left transition-colors",
                  selected
                    ? "bg-[var(--color-bg-surface-hover)]"
                    : "hover:bg-[var(--color-bg-surface-hover)]/70",
                ].join(" ")}
              >
                <span className="relative flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-white/[0.06] bg-white/[0.025] text-[var(--color-text-secondary)] transition-colors group-hover/model:border-white/[0.1]">
                  <ModelBrandIcon model={model} className="h-[18px] w-[18px]" />
                  {selected ? (
                    <span className="absolute -bottom-1 -right-1 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-[var(--color-cherry)] text-[#140709] ring-2 ring-[var(--color-bg-surface-hover)]">
                      <Check size={8} strokeWidth={3.5} />
                    </span>
                  ) : null}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] font-medium text-[var(--color-text-primary)]">
                    {model.label}
                  </span>
                  <span className="mt-0.5 block truncate font-mono text-[10.5px] text-[var(--color-text-tertiary)]">
                    {model.model}
                  </span>
                </span>
              </button>
            );
          })
        ) : (
          <div className="flex min-h-28 flex-col items-center justify-center px-6 text-center">
            <Search size={17} className="mb-2 text-[var(--color-text-muted)]" />
            <div className="text-[12.5px] text-[var(--color-text-secondary)]">
              No {activeGroup?.label ?? "provider"} models match “{search.trim()}”.
            </div>
            <div className="mt-1 text-[11px] text-[var(--color-text-muted)]">
              Try another search or provider.
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

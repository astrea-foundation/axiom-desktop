import { Check, Search, Image, FileText } from "lucide-react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ProviderModel } from "../types";
import { ModelBrandIcon, ProviderBrandIcon } from "./ModelBrandIcon";
import { formatModelPricing } from "./modelPricing";
import { compareModelPreference } from "../modelPreferences";

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
  for (const model of [...models].sort(compareModelPreference)) {
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
      className="glass-panel-solid shadow-glass-pop animate-pop-in absolute left-0 z-40 flex max-h-[min(380px,calc(100vh-4rem))] w-[min(24rem,calc(100vw-3rem))] origin-bottom-left flex-col overflow-hidden rounded-[10px]"
    >
      <div className="border-b border-[var(--color-border)] px-3 pb-0 pt-3">
        <label className="flex h-8 items-center gap-2 rounded-[8px] border border-[var(--color-border)] bg-[var(--color-bg-input)] px-2.5 transition-colors hover:border-[var(--border-hover)] focus-within:border-[var(--color-border-accent)]">
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
          className="mt-2 flex gap-1 overflow-x-auto pb-2"
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
                  "flex shrink-0 items-center gap-1.5 rounded-[8px] px-2.5 py-1.5 text-[12px] transition-colors",
                  active
                    ? "bg-[var(--surface-tile)] text-[var(--color-text-primary)] shadow-glass-tile"
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
                      ? "bg-[var(--color-cherry)] text-on-accent"
                      : "bg-[var(--color-cherry-glow)] text-[var(--color-text-muted)]",
                  ].join(" ")}
                >
                  {group.models.length}
                </span>
              </button>
            );
          })}
        </div>
      </div>

      <div role="tabpanel" className="flex min-h-0 flex-col gap-0.5 overflow-y-auto p-2">
        {activeGroup?.models.length ? (
          activeGroup.models.map((model) => {
            const selected = model.id === selectedModelId;
            const pricing = formatModelPricing(model);
            return (
              <button
                key={model.id}
                type="button"
                onClick={() => onSelect(model.id)}
                className={[
                  "group/model flex w-full items-center gap-2.5 rounded-[8px] px-2 py-1.5 text-left transition-colors hover:bg-[var(--color-bg-surface-hover)]/70",
                ].join(" ")}
              >
                <span className="relative flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-[var(--color-border)] bg-[var(--surface-tile)] text-[var(--color-text-secondary)] transition-colors group-hover/model:border-[var(--color-border-strong)]">
                  <ModelBrandIcon model={model} className="h-[14px] w-[14px]" />
                  {selected ? (
                    <span className="absolute -bottom-1 -right-1 flex h-3 w-3 items-center justify-center rounded-full bg-brand text-white ring-2 ring-[var(--surface-tile)]">
                      <Check size={7} strokeWidth={3.5} />
                    </span>
                  ) : null}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[12.5px] font-medium text-[var(--color-text-primary)]">
                    {model.label}
                    <span className="ml-1.5 inline-flex gap-1 align-middle text-[var(--color-text-tertiary)]">
                      {model.supportsImages ? <span role="img" aria-label="Image uploads" title="Image uploads"><Image size={13} aria-hidden="true" /></span> : null}
                      {model.fileMimeTypes?.length ? <span role="img" aria-label="File uploads" title="File uploads"><FileText size={13} aria-hidden="true" /></span> : null}
                    </span>
                  </span>
                  <span className="mt-0.5 block truncate font-mono text-[10px] text-[var(--color-text-muted)]">
                    {model.model}
                  </span>
                </span>
                <span className="w-[8.25rem] shrink-0 text-right">
                  {pricing ? (
                    <>
                      <span className="block whitespace-nowrap text-[10px] font-medium tabular-nums text-[var(--color-text-secondary)]">
                        {pricing}
                      </span>
                      <span className="mt-0.5 block text-[9.5px] text-[var(--color-text-muted)]">
                        per 1M tokens
                      </span>
                    </>
                  ) : (
                    <span className="block text-[10px] text-[var(--color-text-muted)]">
                      Price unavailable
                    </span>
                  )}
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

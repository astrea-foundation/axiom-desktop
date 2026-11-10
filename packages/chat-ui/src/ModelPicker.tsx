/** @jsxRuntime automatic */
import { Check, FileText, Image, Search } from "lucide-react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ProviderModel } from "./types";
import { ModelBrandIcon, ProviderBrandIcon } from "./ModelBrandIcon";
import { formatModelPricing } from "./modelPricing";
import { compareModelPreference } from "./modelPreferences";
import {
  catalogCapabilities,
  catalogHosts,
  catalogTags,
  countModels,
  hasActiveModelFilters,
  matchesModelFilters,
  toggled,
  type ModelCapability,
  type ModelFilters,
} from "./modelFilters";

interface ModelPickerProps {
  models: ProviderModel[];
  selectedModelId: string;
  onSelect: (modelId: string) => void;
}

const MAX_ROW_TAGS = 3;

const CAPABILITIES: Record<ModelCapability, { label: string; summary: string; Icon: typeof Image }> = {
  images: { label: "Only models that accept images", summary: "Images", Icon: Image },
  files: { label: "Only models that accept files", summary: "Files", Icon: FileText },
};

const chipBase = "flex shrink-0 items-center transition-colors disabled:cursor-default disabled:opacity-40";
const chipActive = "bg-[var(--surface-tile)] text-[var(--color-text-primary)] shadow-glass-tile";
const chipIdle = "text-[var(--color-text-tertiary)] enabled:hover:bg-[var(--color-bg-surface-hover)] enabled:hover:text-[var(--color-text-secondary)]";

function CountBadge({ count, active }: { count: number; active: boolean }) {
  return (
    <span
      className={[
        "min-w-5 rounded-full px-1.5 py-0.5 text-center text-[10px] tabular-nums",
        active
          ? "bg-[var(--color-cherry)] text-on-accent"
          : "bg-[var(--color-cherry-glow)] text-[var(--color-text-muted)]",
      ].join(" ")}
    >
      {count}
    </span>
  );
}

export function ModelPicker({ models, selectedModelId, onSelect }: ModelPickerProps) {
  const pickerRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const downwardOffsetRef = useRef(0);
  const [downwardOffset, setDownwardOffset] = useState(0);
  const [search, setSearch] = useState("");
  const [selectedTags, setSelectedTags] = useState<ReadonlySet<string>>(() => new Set());
  const [selectedHosts, setSelectedHosts] = useState<ReadonlySet<string>>(() => new Set());
  const [selectedCapabilities, setSelectedCapabilities] = useState<ReadonlySet<ModelCapability>>(() => new Set());

  const sorted = useMemo(() => [...models].sort(compareModelPreference), [models]);
  const tagOptions = useMemo(() => catalogTags(sorted), [sorted]);
  const hostOptions = useMemo(() => catalogHosts(sorted), [sorted]);
  const capabilityOptions = useMemo(() => catalogCapabilities(sorted), [sorted]);

  // A catalog refresh can withdraw a tag or host; ignore stale selections
  // instead of silently filtering everything out.
  const filters = useMemo<ModelFilters>(() => ({
    query: search.trim().toLocaleLowerCase(),
    tags: new Set([...selectedTags].filter((id) => tagOptions.some((tag) => tag.id === id))),
    hosts: new Set([...selectedHosts].filter((id) => hostOptions.some((host) => host.id === id))),
    capabilities: new Set([...selectedCapabilities].filter((capability) => capabilityOptions.includes(capability))),
  }), [search, selectedTags, selectedHosts, selectedCapabilities, tagOptions, hostOptions, capabilityOptions]);
  const visible = useMemo(() => sorted.filter((model) => matchesModelFilters(model, filters)), [sorted, filters]);
  const filtering = hasActiveModelFilters(filters);
  const showHosts = hostOptions.length > 1;

  const summary = [
    [
      ...tagOptions.filter((tag) => filters.tags.has(tag.id)).map((tag) => tag.label),
      ...capabilityOptions.filter((capability) => filters.capabilities.has(capability))
        .map((capability) => CAPABILITIES[capability].summary),
    ].join(" + "),
    hostOptions.filter((host) => filters.hosts.has(host.id)).map((host) => host.label).join(" or "),
  ].filter(Boolean).join(" · ");

  const clearFilters = () => {
    setSearch("");
    setSelectedTags(new Set());
    setSelectedHosts(new Set());
    setSelectedCapabilities(new Set());
    searchRef.current?.focus();
  };

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
      className="glass-panel-solid shadow-glass-pop animate-pop-in absolute left-0 z-40 flex max-h-[min(520px,calc(100vh-4rem))] w-[min(30rem,calc(100vw-3rem))] origin-bottom-left flex-col overflow-hidden rounded-[10px]"
    >
      <div className="shrink-0 border-b border-[var(--color-border)] px-3 pb-1 pt-3">
        <label className="flex h-8 items-center gap-2 rounded-[8px] border border-[var(--color-border)] bg-[var(--color-bg-input)] px-2.5 transition-colors hover:border-[var(--border-hover)] focus-within:border-[var(--color-border-accent)]">
          <Search size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
          <input
            ref={searchRef}
            autoFocus
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={tagOptions.length ? "Search models, tags or hosts" : "Search models"}
            aria-label="Search models"
            className="min-w-0 flex-1 bg-transparent text-[12.5px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-tertiary)]"
          />
        </label>

        {tagOptions.length ? (
          <div role="group" aria-label="Filter by tag" className="flex flex-wrap gap-1 pt-2">
            {tagOptions.map((tag) => {
              const active = filters.tags.has(tag.id);
              const count = active ? visible.length : countModels(sorted, filters, { tags: new Set([...filters.tags, tag.id]) });
              return (
                <button
                  key={tag.id}
                  type="button"
                  aria-pressed={active}
                  disabled={!active && count === 0}
                  title={`${count} ${count === 1 ? "model" : "models"}`}
                  data-model-tag-tone={tag.tone}
                  onClick={() => setSelectedTags(toggled(filters.tags, tag.id))}
                  className={[
                    chipBase,
                    "h-6 gap-1 rounded-full pl-[7px] pr-2 text-[11.5px]",
                    active ? chipActive : `${chipIdle} shadow-[inset_0_0_0_1px_var(--color-border)]`,
                  ].join(" ")}
                >
                  <span aria-hidden="true" className="model-tag-dot h-1.5 w-1.5 rounded-full" />
                  {tag.label}
                </button>
              );
            })}
          </div>
        ) : null}

        {showHosts || capabilityOptions.length ? (
          <div className="flex items-center gap-1 pb-1 pt-1.5">
            {showHosts ? (
              <div role="group" aria-label="Filter by host" className="flex min-w-0 flex-wrap gap-1">
                <button
                  type="button"
                  aria-pressed={filters.hosts.size === 0}
                  onClick={() => setSelectedHosts(new Set())}
                  className={[chipBase, "h-7 gap-1.5 rounded-[8px] pl-2.5 pr-1.5 text-[12px]", filters.hosts.size === 0 ? chipActive : chipIdle].join(" ")}
                >
                  All
                  <CountBadge count={countModels(sorted, filters, { hosts: new Set() })} active={filters.hosts.size === 0} />
                </button>
                {hostOptions.map((host) => {
                  const active = filters.hosts.has(host.id);
                  const count = countModels(sorted, filters, { hosts: new Set([host.id]) });
                  return (
                    <button
                      key={host.id}
                      type="button"
                      aria-pressed={active}
                      disabled={!active && count === 0}
                      onClick={() => setSelectedHosts(toggled(filters.hosts, host.id))}
                      className={[chipBase, "h-7 gap-1.5 rounded-[8px] pl-2 pr-1.5 text-[12px]", active ? chipActive : chipIdle].join(" ")}
                    >
                      <ProviderBrandIcon providerId={host.id} providerLabel={host.label} className="h-3.5 w-3.5 shrink-0" />
                      <span>{host.label}</span>
                      <CountBadge count={count} active={active} />
                    </button>
                  );
                })}
              </div>
            ) : null}
            {capabilityOptions.length ? (
              <div role="group" aria-label="Filter by capability" className="ml-auto flex shrink-0 gap-1">
                {capabilityOptions.map((capability) => {
                  const { label, Icon } = CAPABILITIES[capability];
                  const active = filters.capabilities.has(capability);
                  const count = active ? visible.length : countModels(sorted, filters, { capabilities: new Set([...filters.capabilities, capability]) });
                  return (
                    <button
                      key={capability}
                      type="button"
                      aria-pressed={active}
                      aria-label={label}
                      title={label}
                      disabled={!active && count === 0}
                      onClick={() => setSelectedCapabilities(toggled(filters.capabilities, capability))}
                      className={[
                        chipBase,
                        "h-7 w-7 justify-center rounded-[8px]",
                        active ? chipActive : `${chipIdle} shadow-[inset_0_0_0_1px_var(--color-border)]`,
                      ].join(" ")}
                    >
                      <Icon size={13} aria-hidden="true" />
                    </button>
                  );
                })}
              </div>
            ) : null}
          </div>
        ) : null}
      </div>

      <div className="flex shrink-0 items-center justify-between gap-2 px-4 pt-2 text-[11px] text-[var(--color-text-muted)]">
        <span aria-live="polite" className="min-w-0 truncate">
          <span className="font-medium text-[var(--color-text-secondary)]">{visible.length}</span>
          {" "}{visible.length === 1 ? "model" : "models"}{summary ? ` · ${summary}` : ""}
        </span>
        {filtering ? (
          <button
            type="button"
            onClick={clearFilters}
            className="shrink-0 text-[var(--color-text-secondary)] transition-colors hover:text-[var(--color-text-primary)] hover:underline"
          >
            Clear filters
          </button>
        ) : null}
      </div>

      <div role="group" aria-label="Models" className="flex min-h-0 flex-col gap-0.5 overflow-y-auto p-2 pt-1.5">
        {visible.length ? (
          visible.map((model) => {
            const selected = model.id === selectedModelId;
            const pricing = formatModelPricing(model);
            const tags = model.tags ?? [];
            const hiddenTags = tags.length - MAX_ROW_TAGS;
            return (
              <button
                key={model.id}
                type="button"
                title={model.model}
                aria-current={selected ? "true" : undefined}
                onClick={() => onSelect(model.id)}
                className="group/model flex w-full items-center gap-2.5 rounded-[8px] px-2 py-1.5 text-left transition-colors hover:bg-[var(--color-bg-surface-hover)]/70"
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
                  <span className="mt-0.5 flex min-w-0 items-center gap-1.5 whitespace-nowrap text-[10.5px] text-[var(--color-text-muted)]">
                    <ProviderBrandIcon providerId={model.providerId} providerLabel={model.providerLabel} className="h-[11px] w-[11px] shrink-0" />
                    <span className="shrink-0">{model.providerLabel}</span>
                    {tags.length ? (
                      <span className="flex min-w-0 gap-[3px] overflow-hidden pl-0.5">
                        {tags.slice(0, MAX_ROW_TAGS).map((tag) => (
                          <span
                            key={tag.id}
                            data-model-tag-tone={tag.tone}
                            className="model-tag-pill inline-flex h-[15px] shrink-0 items-center rounded-full px-1.5 text-[9.5px] font-semibold"
                          >
                            {tag.label}
                          </span>
                        ))}
                        {hiddenTags > 0 ? (
                          <span
                            data-model-tag-tone="gray"
                            title={tags.slice(MAX_ROW_TAGS).map((tag) => tag.label).join(", ")}
                            className="model-tag-pill inline-flex h-[15px] shrink-0 items-center rounded-full px-1.5 text-[9.5px] font-semibold"
                          >
                            +{hiddenTags}
                          </span>
                        ) : null}
                      </span>
                    ) : null}
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
              {filters.query ? `No models match “${search.trim()}”.` : "No models match these filters."}
            </div>
            {filtering ? (
              <button
                type="button"
                onClick={clearFilters}
                className="mt-1 text-[11px] text-[var(--color-text-muted)] underline transition-colors hover:text-[var(--color-text-secondary)]"
              >
                Clear filters
              </button>
            ) : null}
          </div>
        )}
      </div>
    </div>
  );
}

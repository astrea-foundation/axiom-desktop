/** @jsxRuntime automatic */
import { Check, FileText, Image, Search } from "lucide-react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ProviderModel } from "./types";
import { ModelBrandIcon } from "./ModelBrandIcon";
import { formatModelPricing } from "./modelPricing";
import { compareModelGroups, groupModels, modelGroupKey } from "./modelGroups";
import {
  catalogCapabilities,
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
  /** Models whose provider is temporarily down: listed greyed out, never selectable. */
  unavailableModels?: ProviderModel[];
  selectedModelId: string;
  /**
   * Receives an exact catalog offering. Each model is one row however many
   * providers serve it: the selected model keeps its current provider, and any
   * other row offers its preferred provider (see `compareOfferings`).
   */
  onSelect: (modelId: string) => void;
}

const MAX_ROW_TAGS = 3;
const MAX_HEIGHT = 520;
const MAX_WIDTH = 480;
const CRAMPED_HEIGHT = 300;
/** Gap kept between the picker and whatever would clip it. */
const EDGE_GAP = 8;
/** Desktop's window title bar; hosts without one are bounded by their own containers. */
const SAFE_TOP = 52;

interface Placement {
  /** Pixels moved down and right from the anchored position above the model control. */
  down: number;
  right: number;
  maxHeight: number | null;
  maxWidth: number | null;
}

const UNPLACED: Placement = { down: 0, right: 0, maxHeight: null, maxWidth: null };

/**
 * The viewport rectangle the picker can occupy without being cut off: the
 * window, narrowed by every ancestor that clips or scrolls its overflow.
 */
function visibleBounds(element: HTMLElement) {
  let top = SAFE_TOP - EDGE_GAP;
  let left = 0;
  let right = document.documentElement.clientWidth;
  let bottom = window.innerHeight;
  for (let node = element.parentElement; node; node = node.parentElement) {
    const style = window.getComputedStyle(node);
    const clipsX = style.overflowX !== "visible";
    const clipsY = style.overflowY !== "visible";
    if (!clipsX && !clipsY) continue;
    const rect = node.getBoundingClientRect();
    if (clipsX) {
      left = Math.max(left, rect.left + node.clientLeft);
      right = Math.min(right, rect.left + node.clientLeft + node.clientWidth);
    }
    if (clipsY) {
      top = Math.max(top, rect.top + node.clientTop);
      bottom = Math.min(bottom, rect.top + node.clientTop + node.clientHeight);
    }
  }
  return { top: top + EDGE_GAP, left: left + EDGE_GAP, right: right - EDGE_GAP, bottom: bottom - EDGE_GAP };
}

const CAPABILITIES: Record<ModelCapability, { label: string; summary: string; Icon: typeof Image }> = {
  images: { label: "Only models that accept images", summary: "Images", Icon: Image },
  files: { label: "Only models that accept files", summary: "Files", Icon: FileText },
};

const chipBase = "flex shrink-0 items-center transition-colors disabled:cursor-default disabled:opacity-40";
const chipActive = "bg-[var(--surface-tile)] text-[var(--color-text-primary)] shadow-glass-tile";
const chipIdle = "text-[var(--color-text-tertiary)] enabled:hover:bg-[var(--color-bg-surface-hover)] enabled:hover:text-[var(--color-text-secondary)] shadow-[inset_0_0_0_1px_var(--color-border)]";

function ModelRow({ model, selected, unavailable, onSelect }: {
  model: ProviderModel;
  selected: boolean;
  unavailable: boolean;
  onSelect: (modelId: string) => void;
}) {
  const pricing = formatModelPricing(model);
  const tags = model.tags ?? [];
  const hiddenTags = tags.length - MAX_ROW_TAGS;
  const dimmed = unavailable ? "opacity-45 grayscale" : "";
  return (
    <button
      type="button"
      title={unavailable ? "Temporarily unavailable" : model.model}
      aria-current={selected ? "true" : undefined}
      disabled={unavailable}
      data-model-unavailable={unavailable ? "true" : undefined}
      onClick={() => onSelect(model.id)}
      className="group/model flex w-full items-center gap-2.5 rounded-[8px] px-2 py-1.5 text-left transition-colors enabled:hover:bg-[var(--color-bg-surface-hover)]/70 disabled:cursor-not-allowed"
    >
      <span className={`relative flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-[var(--color-border)] bg-[var(--surface-tile)] text-[var(--color-text-secondary)] transition-colors group-enabled/model:group-hover/model:border-[var(--color-border-strong)] ${dimmed}`}>
        <ModelBrandIcon model={model} className="h-[14px] w-[14px]" />
        {selected ? (
          <span className="absolute -bottom-1 -right-1 flex h-3 w-3 items-center justify-center rounded-full bg-brand text-white ring-2 ring-[var(--surface-tile)]">
            <Check size={7} strokeWidth={3.5} />
          </span>
        ) : null}
      </span>
      <span className={`min-w-0 flex-1 ${dimmed}`}>
        <span className="block truncate text-[12.5px] font-medium text-[var(--color-text-primary)]">
          {model.label}
          <span className="ml-1.5 inline-flex gap-1 align-middle text-[var(--color-text-tertiary)]">
            {model.supportsImages ? <span role="img" aria-label="Image uploads" title="Image uploads"><Image size={13} aria-hidden="true" /></span> : null}
            {model.fileMimeTypes?.length ? <span role="img" aria-label="File uploads" title="File uploads"><FileText size={13} aria-hidden="true" /></span> : null}
          </span>
        </span>
        {tags.length ? (
          <span className="mt-0.5 flex min-w-0 gap-[3px] overflow-hidden whitespace-nowrap">
            {tags.slice(0, MAX_ROW_TAGS).map((tag) => (
              <span key={tag.id} className="model-tag-pill inline-flex h-[15px] shrink-0 items-center rounded-full px-1.5 text-[9.5px] font-medium">
                {tag.label}
              </span>
            ))}
            {hiddenTags > 0 ? (
              <span
                title={tags.slice(MAX_ROW_TAGS).map((tag) => tag.label).join(", ")}
                className="model-tag-pill inline-flex h-[15px] shrink-0 items-center rounded-full px-1.5 text-[9.5px] font-medium"
              >
                +{hiddenTags}
              </span>
            ) : null}
          </span>
        ) : null}
      </span>
      <span className="w-[8.25rem] shrink-0 text-right">
        {unavailable ? (
          <span className="block text-[10px] text-[var(--color-text-muted)]">
            Temporarily unavailable
          </span>
        ) : pricing ? (
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
}

export function ModelPicker({ models, unavailableModels, selectedModelId, onSelect }: ModelPickerProps) {
  const pickerRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [placement, setPlacement] = useState<Placement>(UNPLACED);
  const [search, setSearch] = useState("");
  const [selectedTags, setSelectedTags] = useState<ReadonlySet<string>>(() => new Set());
  const [selectedCapabilities, setSelectedCapabilities] = useState<ReadonlySet<ModelCapability>>(() => new Set());

  // One row per model. The selected model shows the offering in use; every
  // other row shows the provider it would get if chosen now.
  const selectedKey = useMemo(() => {
    const selected = [...models, ...(unavailableModels ?? [])].find((model) => model.id === selectedModelId);
    return selected ? modelGroupKey(selected) : null;
  }, [models, unavailableModels, selectedModelId]);
  const groups = useMemo(() => groupModels(models).sort(compareModelGroups), [models]);
  const sorted = useMemo(() => groups.map((group) => (
    group.key === selectedKey ? group.offerings.find((model) => model.id === selectedModelId) : undefined
  ) ?? group.offerings[0]!), [groups, selectedKey, selectedModelId]);
  const tagOptions = useMemo(() => catalogTags(sorted), [sorted]);
  // A model whose every provider is down keeps its row, greyed out, instead of vanishing.
  const unavailable = useMemo(() => {
    const available = new Set(groups.map((group) => group.key));
    return groupModels((unavailableModels ?? []).filter((model) => !available.has(modelGroupKey(model))))
      .sort(compareModelGroups)
      .map((group) => group.offerings[0]!);
  }, [groups, unavailableModels]);
  const capabilityOptions = useMemo(() => catalogCapabilities(sorted), [sorted]);

  // A catalog refresh can withdraw a tag; ignore stale selections instead of
  // silently filtering everything out.
  const filters = useMemo<ModelFilters>(() => ({
    query: search.trim().toLocaleLowerCase(),
    tags: new Set([...selectedTags].filter((id) => tagOptions.some((tag) => tag.id === id))),
    capabilities: new Set([...selectedCapabilities].filter((capability) => capabilityOptions.includes(capability))),
  }), [search, selectedTags, selectedCapabilities, tagOptions, capabilityOptions]);
  const visible = useMemo(() => sorted.filter((model) => matchesModelFilters(model, filters)), [sorted, filters]);
  const visibleUnavailable = useMemo(() => unavailable.filter((model) => matchesModelFilters(model, filters)), [unavailable, filters]);
  const filtering = hasActiveModelFilters(filters);

  const summary = [
    ...tagOptions.filter((tag) => filters.tags.has(tag.id)).map((tag) => tag.label),
    ...capabilityOptions.filter((capability) => filters.capabilities.has(capability))
      .map((capability) => CAPABILITIES[capability].summary),
  ].join(" + ");

  const clearFilters = () => {
    setSearch("");
    setSelectedTags(new Set());
    setSelectedCapabilities(new Set());
    searchRef.current?.focus();
  };

  useLayoutEffect(() => {
    const picker = pickerRef.current;
    if (!picker) return;
    let frame = 0;
    const measure = () => {
      const bounds = visibleBounds(picker);
      const parent = picker.offsetParent?.getBoundingClientRect();
      // Layout offsets exclude both the placement translation and the pop-in animation.
      const naturalTop = (parent?.top ?? 0) + picker.offsetTop;
      const naturalLeft = (parent?.left ?? 0) + picker.offsetLeft;
      const maxHeight = Math.max(0, Math.floor(Math.min(MAX_HEIGHT, bounds.bottom - bounds.top)));
      const maxWidth = Math.max(0, Math.floor(Math.min(MAX_WIDTH, bounds.right - bounds.left)));
      // Keep the anchored position when it fits, otherwise slide to the nearest spot that does.
      const lowest = Math.max(bounds.top, bounds.bottom - Math.min(picker.offsetHeight, maxHeight));
      const rightmost = Math.max(bounds.left, bounds.right - Math.min(picker.offsetWidth, maxWidth));
      const next = {
        down: Math.round(Math.min(Math.max(naturalTop, bounds.top), lowest) - naturalTop),
        right: Math.round(Math.min(Math.max(naturalLeft, bounds.left), rightmost) - naturalLeft),
        maxHeight,
        maxWidth,
      };
      setPlacement(current => next.down === current.down && next.right === current.right &&
        next.maxHeight === current.maxHeight && next.maxWidth === current.maxWidth ? current : next);
    };
    const queueMeasurement = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(measure);
    };
    const observer = new ResizeObserver(queueMeasurement);
    observer.observe(picker);
    for (let node = picker.parentElement; node; node = node.parentElement) observer.observe(node);
    window.addEventListener("resize", queueMeasurement);
    window.addEventListener("scroll", queueMeasurement, true);
    searchRef.current?.focus({ preventScroll: true });
    measure();
    return () => {
      window.cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", queueMeasurement);
      window.removeEventListener("scroll", queueMeasurement, true);
    };
  }, []);

  // Too short for a fixed header above a scrolling list: scroll the whole picker instead.
  const cramped = placement.maxHeight !== null && placement.maxHeight < CRAMPED_HEIGHT;

  return (
    <div
      ref={pickerRef}
      role="dialog"
      aria-label="Choose a model"
      style={{
        translate: `${placement.right}px ${placement.down}px`,
        ...(placement.maxHeight === null ? {} : { maxHeight: placement.maxHeight }),
        ...(placement.maxWidth === null ? {} : { maxWidth: placement.maxWidth }),
      }}
      className={`glass-panel-solid shadow-glass-pop animate-pop-in absolute bottom-10 left-0 z-40 flex max-h-[min(520px,calc(100vh-4rem))] w-[min(30rem,calc(100vw-3rem))] origin-bottom-left flex-col rounded-[10px] ${cramped ? "overflow-y-auto" : "overflow-hidden"}`}
    >
      <div className="shrink-0 border-b border-[var(--color-border)] px-3 pb-2 pt-3">
        <label className="flex h-8 items-center gap-2 rounded-[8px] border border-[var(--color-border)] bg-[var(--color-bg-input)] px-2.5 transition-colors hover:border-[var(--border-hover)] focus-within:border-[var(--color-border-accent)]">
          <Search size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
          <input
            ref={searchRef}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={tagOptions.length ? "Search models or tags" : "Search models"}
            aria-label="Search models"
            className="min-w-0 flex-1 bg-transparent text-[12.5px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-tertiary)]"
          />
        </label>

        {tagOptions.length || capabilityOptions.length ? (
          <div className="flex items-start gap-2 pt-2">
            {tagOptions.length ? (
              <div role="group" aria-label="Filter by tag" className="flex min-w-0 flex-1 flex-wrap gap-1">
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
                      onClick={() => setSelectedTags(toggled(filters.tags, tag.id))}
                      className={[chipBase, "h-6 rounded-full px-2.5 text-[11.5px]", active ? chipActive : chipIdle].join(" ")}
                    >
                      {tag.label}
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
                      className={[chipBase, "h-6 w-6 justify-center rounded-full", active ? chipActive : chipIdle].join(" ")}
                    >
                      <Icon size={12} aria-hidden="true" />
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
          {" "}{visible.length === 1 ? "model" : "models"}{summary ? `: ${summary}` : ""}
          {visibleUnavailable.length ? `, ${visibleUnavailable.length} temporarily unavailable` : ""}
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

      <div role="group" aria-label="Models" className={`flex flex-col gap-0.5 p-2 pt-1.5 ${cramped ? "shrink-0" : "min-h-0 overflow-y-auto"}`}>
        {visible.length + visibleUnavailable.length ? (
          <>
            {visible.map((model) => (
              <ModelRow key={model.id} model={model} selected={model.id === selectedModelId} unavailable={false} onSelect={onSelect} />
            ))}
            {visibleUnavailable.map((model) => (
              <ModelRow key={model.id} model={model} selected={model.id === selectedModelId} unavailable onSelect={onSelect} />
            ))}
          </>
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

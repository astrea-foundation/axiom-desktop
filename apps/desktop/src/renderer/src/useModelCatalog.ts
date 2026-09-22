import type { ClientState, ListModelsResponse } from "@axiom/axiom-acp-client";
import { useCallback, useEffect, useRef, useState } from "react";
import { desktopErrorMessage } from "./signInFlow";
import type { ProviderModel, ReasoningEffort } from "./types";
import { compareModelPreference } from "./modelPreferences";

import { explicitReasoningEfforts, isReasoningEffort, reconcileReasoningEffort } from "./reasoningSettings";

import { type useConversationNavigation } from "./useConversationNavigation";

function withoutProviderSuffix(value: string, providerId: string, providerLabel: string): string {
  const original = value.trim();
  for (const provider of new Set([providerLabel.trim(), providerId.trim()])) {
    if (!provider) continue;
    for (const suffix of [
      ` · ${provider}`,
      ` • ${provider}`,
      ` — ${provider}`,
      ` – ${provider}`,
      ` - ${provider}`,
      ` | ${provider}`,
      ` (${provider})`,
    ]) {
      if (original.toLocaleLowerCase().endsWith(suffix.toLocaleLowerCase())) {
        return original.slice(0, -suffix.length).trim() || original;
      }
    }
  }
  return original;
}

function providerModels(response: ListModelsResponse): ProviderModel[] {
  return response.models.map((model) => {
    const providerId = model.providerId.trim();
    const providerLabel = model.providerLabel.trim();
    if (
      !model.id.trim()
      || !model.label.trim()
      || !model.shortLabel.trim()
      || !providerId
      || !providerLabel
      || !model.upstreamModel.trim()
    ) throw new Error("AxiomCLI returned incomplete provider model metadata");
    const label = withoutProviderSuffix(model.label, providerId, providerLabel);
    const shortLabel = withoutProviderSuffix(model.shortLabel, providerId, providerLabel);
    const inputPrice = model.inputPriceMicrousdPerMillionTokens ?? null;
    const outputPrice = model.outputPriceMicrousdPerMillionTokens ?? null;
    const validPrice = (value: number | null): boolean => value === null
      || (Number.isSafeInteger(value) && value > 0);
    if (
      !validPrice(inputPrice)
      || !validPrice(outputPrice)
      || (inputPrice === null) !== (outputPrice === null)
    ) throw new Error("AxiomCLI returned invalid provider pricing metadata");
    return {
      id: model.id,
      label,
      shortLabel,
      providerId,
      providerLabel,
      model: model.upstreamModel.trim(),
      supportedReasoningEfforts: explicitReasoningEfforts(model.thinkingLevels.filter(isReasoningEffort)),
      inputPriceMicrousdPerMillionTokens: inputPrice,
      outputPriceMicrousdPerMillionTokens: outputPrice,
      contextWindowTokens: model.contextWindowTokens,
      supportsImages: model.supportsImages,
      fileMimeTypes: model.fileMimeTypes,
      autoCompactThresholdTokens: model.autoCompactThresholdTokens,
    };
  }).sort(compareModelPreference);
}

export function useModelCatalog(agentState: ClientState, navigation: ReturnType<typeof useConversationNavigation>, setUiError: (error: string | null) => void) {
  const { surface, activeThreadId, activeThreadIdRef } = navigation;
  const [models, setModels] = useState<ProviderModel[]>([]);
  const [modelId, setModelId] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("provider_default");
  const [modelCatalogError, setModelCatalogError] = useState<string | null>(null);
  const [modelCatalogRefresh, setModelCatalogRefresh] = useState(0);
  const [desktopReady, setDesktopReady] = useState(false);
  const settingsGeneration = useRef(0);
  const catalogScope = useRef("");
  const preferredModelId = useRef("");
  const preferredReasoningEffort = useRef<ReasoningEffort>("provider_default");
  const selectedModel = models.find((model) => model.id === modelId) ?? (modelId ? undefined : models[0]);
  const effectiveReasoningEffort = reconcileReasoningEffort(reasoningEffort, selectedModel?.supportedReasoningEfforts ?? []);

  useEffect(() => {
    if (!selectedModel) return;
    setReasoningEffort(effectiveReasoningEffort);
    if (surface !== "thread") {
      setModelId(selectedModel.id);
      preferredModelId.current = selectedModel.id;
      preferredReasoningEffort.current = effectiveReasoningEffort;
    }
  }, [selectedModel, reasoningEffort, effectiveReasoningEffort, surface]);

  const activeSession = activeThreadId ? agentState.sessions[activeThreadId] ?? null : null;

  useEffect(() => {
    if (!agentState.connected) {
      setDesktopReady(false);
      return;
    }
    const api = window.axiomDesktop?.agent;
    if (!api) return;
    let cancelled = false;
    const initialize = async () => {
      setDesktopReady(false);
      try {
        const bootstrap = await api.desktopBootstrap();
        if (cancelled) return;
        const rememberedModel = bootstrap.newThreadSettings.model === "auto" ? "" : bootstrap.newThreadSettings.model ?? "";
        const rememberedThinking = bootstrap.newThreadSettings.thinkingLevel;
        const nextThinking = isReasoningEffort(rememberedThinking) ? rememberedThinking : "provider_default";
        setModelId(rememberedModel);
        setReasoningEffort(nextThinking);
        preferredModelId.current = rememberedModel;
        preferredReasoningEffort.current = nextThinking;
        setDesktopReady(true);
        setUiError(null);
      } catch (error) {
        if (!cancelled) setUiError(error instanceof Error ? error.message : String(error));
      }
    };
    void initialize();
    return () => { cancelled = true; };
  }, [agentState.connected, agentState.runtimeInstanceId]);

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected || !desktopReady || agentState.account?.state !== "valid") {
      setModels([]);
      setModelCatalogError(null);
      return;
    }
    let cancelled = false;
    const scope = `${agentState.runtimeInstanceId}:${agentState.account.account?.id}`;
    if (catalogScope.current !== scope) setModels([]);
    catalogScope.current = scope;
    setModelCatalogError(null);
    void api.listModels().then((response) => {
      if (cancelled) return;
      const available = providerModels(response);
      if (available.length === 0) throw new Error("this account has no available models");
      const rememberedModel = preferredModelId.current;
      const nextModel = rememberedModel || available[0]!.id;
      setModels(available);
      if (!activeThreadIdRef.current) {
        const thinking = reconcileReasoningEffort(preferredReasoningEffort.current, available.find((model) => model.id === nextModel)?.supportedReasoningEfforts ?? []);
        setModelId(nextModel);
        setReasoningEffort(thinking);
        preferredModelId.current = nextModel;
        preferredReasoningEffort.current = thinking;
      }
    }).catch((error: unknown) => {
      if (cancelled) return;
      setModels([]);
      const detail = desktopErrorMessage(error, "AxiomCLI could not load the model catalog.");
      setModelCatalogError(`Could not load models. Check your connection or refresh your account in Settings. ${detail}`);
    });
    return () => { cancelled = true; };
  }, [
    agentState.account?.account?.id,
    agentState.account?.state,
    agentState.connected,
    agentState.runtimeInstanceId,
    desktopReady,
    modelCatalogRefresh,
  ]);

  useEffect(() => {
    if (!agentState.connected || agentState.account?.state !== "valid") return;
    const refresh = () => setModelCatalogRefresh((value) => value + 1);
    const timer = window.setInterval(refresh, 5 * 60 * 1000);
    window.addEventListener("focus", refresh);
    return () => { window.clearInterval(timer); window.removeEventListener("focus", refresh); };
  }, [agentState.connected, agentState.account?.state]);

  useEffect(() => {
    if (!activeSession?.settings) return;
    setModelId(activeSession.settings.model);
    const level = activeSession.settings.thinkingLevel;
    setReasoningEffort(isReasoningEffort(level) ? level : "provider_default");
  }, [activeSession?.settings]);

  useEffect(() => {
    settingsGeneration.current += 1;
  }, [activeThreadId]);

  const updateSettings = (nextModel?: string, nextThinking?: ReasoningEffort) => {
    if (!activeThreadId || !activeSession) return;
    const api = window.axiomDesktop?.agent;
    if (!api) return;
    const generation = ++settingsGeneration.current;
    const selected = models.find((model) => model.id === (nextModel ?? modelId));
    if (!selected) return;
    const thinking = reconcileReasoningEffort(nextThinking ?? reasoningEffort, selected.supportedReasoningEfforts);
    if (nextModel) setModelId(nextModel);
    setReasoningEffort(thinking);
    void api.setSettings({
      threadId: activeThreadId,
      ...(nextModel ? { model: nextModel } : {}),
      thinkingLevel: thinking,
    }).then((result) => {
      if (generation !== settingsGeneration.current || result.threadId !== activeThreadId) return;
      const level = result.settings.thinkingLevel;
      setModelId(result.settings.model);
      setReasoningEffort(isReasoningEffort(level) ? level : "provider_default");
      preferredModelId.current = result.settings.model;
      preferredReasoningEffort.current = isReasoningEffort(level) ? level : "provider_default";
      setUiError(null);
    }).catch((error: unknown) => {
      if (generation !== settingsGeneration.current) return;
      if (activeSession.settings) {
        setModelId(activeSession.settings.model);
        const level = activeSession.settings.thinkingLevel;
        setReasoningEffort(isReasoningEffort(level) ? level : "provider_default");
      }
      setUiError(error instanceof Error ? error.message : String(error));
    });
  };

  const modelStatusLabel = models.length && modelId && !selectedModel ? "Model unavailable" : !agentState.connected || !desktopReady
    ? "Starting…"
    : modelCatalogError
      ? "Models unavailable"
      : agentState.account?.state === "expired"
        ? "Sign in again"
        : agentState.account?.state === "unavailable"
          ? "Models unavailable"
          : agentState.account?.state === "valid"
            ? "Loading models…"
            : "Sign in for models";
  const refreshModels = useCallback(() => setModelCatalogRefresh((version) => version + 1), []);
  const clearModels = useCallback(() => { setModels([]); setModelCatalogError(null); }, []);
  const rememberSelection = useCallback((model: string, thinking: ReasoningEffort) => {
    preferredModelId.current = model;
    preferredReasoningEffort.current = thinking;
  }, []);
  const restorePreferredSelection = useCallback(() => {
    setModelId(preferredModelId.current);
    setReasoningEffort(preferredReasoningEffort.current);
  }, []);
  const selectNewModel = (id: string) => {
    const selected = models.find((model) => model.id === id);
    if (!selected) return;
    const thinking = reconcileReasoningEffort(reasoningEffort, selected.supportedReasoningEfforts);
    setModelId(id);
    setReasoningEffort(thinking);
    rememberSelection(id, thinking);
  };
  const selectNewThinking = (effort: ReasoningEffort) => {
    const thinking = reconcileReasoningEffort(effort, selectedModel?.supportedReasoningEfforts ?? []);
    setReasoningEffort(thinking);
    preferredReasoningEffort.current = thinking;
  };
  return { models, modelId, selectedModel, effectiveReasoningEffort, desktopReady, modelCatalogError, modelStatusLabel, updateSettings, refreshModels, clearModels, rememberSelection, restorePreferredSelection, selectNewModel, selectNewThinking };
}

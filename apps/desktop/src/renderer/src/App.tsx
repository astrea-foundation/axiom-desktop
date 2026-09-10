import { Folder, PanelLeftOpen } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ClientState, ListModelsResponse } from "@axiom/axiom-acp-client";
import { ChatView } from "./components/ChatView";
import { PreparingThreadView } from "./components/PreparingThreadView";
import { OverflowTitle } from "./components/OverflowTitle";
import { DeleteConfirmationDialog } from "./components/DeleteConfirmationDialog";
import { RenameDialog } from "./components/RenameDialog";
import { ProxyView } from "./components/ProxyView";
import { BalanceScreen } from "./components/BalanceScreen";
import { SettingsSheet } from "./components/SettingsSheet";
import type { SettingsCategory } from "./components/SettingsPanel";
import { UpdateNotice } from "./components/UpdateNotice";
import { useDesktopUpdates } from "./useDesktopUpdates";
import { SignInStatus } from "./components/SignInStatus";
import { Sidebar } from "./components/Sidebar";
import { TrafficLightGhost } from "./components/TrafficLightGhost";
import { WelcomeView } from "./components/WelcomeView";
import { WindowControls } from "./components/WindowControls";
import {
  BillingInvoiceRequestTracker,
  browserInvoiceRequestStorage,
} from "./billingInvoiceRequest";
import { BillingCheckoutFlow, monitorBillingInvoice } from "./billingCheckout";
import { ThreadLoadTracker } from "./threadLoading";
import { compareThreadMessages, newestMessageAt, orderFoldersByMessages } from "./threadOrdering";
import { setThemePreference, useTheme } from "./lib/theme";
import { useWebAccess } from "./useWebAccess";
import { useAccountDraft } from "./useAccountDraft";
import { MessageQueue as MessageQueueController } from "./messageQueue";
import { MessageQueue } from "./components/MessageQueue";
import { browserConsentStorage } from "./webConsent";
import { WebConsentDialog } from "./components/WebConsentDialog";
import { DEFAULT_AGENT_SELECTION, type AgentSelection, type AgentControlOptions } from "./components/AgentModeControl";
import { isMac, MAC_TRAFFIC_LIGHT_INSET_CLASS, useFullScreen } from "./lib/window-chrome";
import { accountPresentation, desktopErrorMessage, signInErrorMessage, type SignInState } from "./signInFlow";
import type { MainSurface, PendingUserMessage, ProviderModel, ReasoningEffort, Thread } from "./types";

import { explicitReasoningEfforts, isReasoningEffort, reconcileReasoningEffort } from "./reasoningSettings";

const SIDEBAR_WIDTH = 260;

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

const EMPTY_AGENT_STATE: ClientState = {
  connected: false,
  runtimeInstanceId: null,
  lastSequence: 0,
  sessions: {},
  catalog: [],
  collections: { revision: 0, collections: [] },
  preferences: null,
  account: null,
  billing: null,
  invoice: null,
  diagnostic: "",
  error: null,
};

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
      autoCompactThresholdTokens: model.autoCompactThresholdTokens,
    };
  });
}

function relativeTime(value: string): string {
  const elapsed = Date.now() - Date.parse(value);
  if (!Number.isFinite(elapsed) || elapsed < 60_000) return "just now";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}m`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}h`;
  return `${Math.floor(elapsed / 86_400_000)}d`;
}

export function App() {
  const updates = useDesktopUpdates();
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>("appearance");
  const [agentState, setAgentState] = useState<ClientState>(EMPTY_AGENT_STATE);
  const [models, setModels] = useState<ProviderModel[]>([]);
  const [surface, setSurface] = useState<MainSurface>("welcome");
  const [activeThreadId, setActiveThreadId] = useState<string | null>(null);
  const activeThreadIdRef = useRef(activeThreadId);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const fullScreen = useFullScreen();
  useTheme();
  const [draft, setDraft] = useAccountDraft(agentState.account?.state === "valid" ? agentState.account.account?.id ?? null : null);
  const [modelId, setModelId] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("provider_default");
  const [accountScreen, setAccountScreen] = useState<"settings" | "balance" | null>(null);
  const accountScreenOpen = accountScreen !== null;
  const accountScreenReturnFocus = useRef<HTMLElement | null>(null);
  const [proxyInspecting, setProxyInspecting] = useState(false);
  const [uiError, setUiError] = useState<string | null>(null);
  const [queueRevision, setQueueRevision] = useState(0);
  const [newAgentDraft, setNewAgentDraft] = useState<{ scope: string; selection: AgentSelection } | null>(null);
  const [agentSettingsBusy, setAgentSettingsBusy] = useState(false);
  const messageQueue = useRef<MessageQueueController | null>(null);
  const submittingDraft = useRef<string | null>(null);
  const [threadPreparation, setThreadPreparation] = useState<{ id: string; text: string; accountId: string | null; runtimeId: string | null } | null>(null);
  const cancelThreadPreparation = useCallback(() => {
    submittingDraft.current = null;
    setThreadPreparation(null);
  }, []);
  if (!messageQueue.current) messageQueue.current = new MessageQueueController(browserConsentStorage(), () => setQueueRevision((value) => value + 1), setUiError);
  const [modelCatalogError, setModelCatalogError] = useState<string | null>(null);
  const [modelCatalogRefresh, setModelCatalogRefresh] = useState(0);
  const [desktopReady, setDesktopReady] = useState(false);
  const [pendingUserMessage, setPendingUserMessage] = useState<PendingUserMessage | null>(null);
  const [signInState, setSignInState] = useState<SignInState>({ kind: "closed" });
  const threadLoads = useRef(new ThreadLoadTracker());
  const [threadLoadError, setThreadLoadError] = useState<{ id: string; detail: string } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ kind: "thread" | "folder"; id: string; title: string } | null>(null);
  const [renameTarget, setRenameTarget] = useState<{ kind: "thread" | "folder"; id: string; title: string } | null>(null);
  const seenMessageTimes = useRef(new Map<string, number>());
  const settingsGeneration = useRef(0);
  const preferredModelId = useRef("");
  const preferredReasoningEffort = useRef<ReasoningEffort>("provider_default");
  const selectedModel = models.find((model) => model.id === modelId) ?? models[0];
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

  const signInGeneration = useRef(0);
  const signInPending = useRef(false);
  const activeLoginId = useRef<string | null>(null);
  const signInCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const billingInvoiceRequests = useRef<BillingInvoiceRequestTracker | null>(null);
  const billingCheckout = useRef<BillingCheckoutFlow | null>(null);
  const billingMonitor = useRef<ReturnType<typeof monitorBillingInvoice> | null>(null);
  const [billingMonitorRevision, setBillingMonitorRevision] = useState(0);
  if (billingInvoiceRequests.current === null) {
    billingInvoiceRequests.current = new BillingInvoiceRequestTracker(
      () => crypto.randomUUID(),
      browserInvoiceRequestStorage(),
    );
    billingCheckout.current = new BillingCheckoutFlow(billingInvoiceRequests.current);
  }

  const billingAccountId = agentState.account?.state === "valid"
    ? agentState.account.account?.id ?? null
    : null;
  const webAccess = useWebAccess(
    billingAccountId, agentState.runtimeInstanceId, agentState.connected,
    surface === "thread" && activeThreadId ? activeThreadId : "new",
  );
  const resetNewWeb = webAccess.resetNew;
  useEffect(() => {
    messageQueue.current!.sync(agentState, window.axiomDesktop?.agent ?? null);
  }, [agentState, queueRevision]);
  useEffect(() => () => messageQueue.current?.dispose(), []);
  useEffect(() => {
    const definitiveAccount = agentState.account?.state === "valid"
      ? billingAccountId
      : agentState.account?.state === "signed_out" || agentState.account?.state === "expired"
        ? null
        : undefined;
    billingInvoiceRequests.current?.reconcileAccount(definitiveAccount);
  }, [agentState.account?.state, billingAccountId]);

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    const invoice = agentState.invoice;
    if (!api || !agentState.connected || !billingAccountId || !invoice) return;
    const monitor = monitorBillingInvoice(invoice, api);
    billingMonitor.current = monitor;
    const refreshOnReturn = () => { void monitor.refresh(); };
    window.addEventListener("focus", refreshOnReturn);
    return () => {
      void monitor.stop();
      if (billingMonitor.current === monitor) billingMonitor.current = null;
      window.removeEventListener("focus", refreshOnReturn);
    };
  }, [agentState.connected, agentState.runtimeInstanceId, billingAccountId,
    agentState.invoice?.id, agentState.invoice?.state, billingMonitorRevision]);

  const unsortedFolders = useMemo(() => agentState.collections.collections.map((collection) => ({
    id: collection.id,
    name: collection.name,
    collapsed: collection.collapsed,
  })), [agentState.collections]);

  const collectionAssignments = useMemo(() => Object.fromEntries(
    agentState.collections.collections.flatMap((collection) =>
      collection.threadIds.map((threadId) => [threadId, collection.id] as const),
    ),
  ), [agentState.collections]);

  const threads = useMemo<Thread[]>(() => {
    const catalogIds = new Set(agentState.catalog.map((summary) => summary.threadId));
    const summaries = [
      ...agentState.catalog,
      ...Object.values(agentState.sessions)
        .filter((session) => !catalogIds.has(session.sessionId))
        .map((session) => ({
          threadId: session.sessionId,
          title: session.title,
          cwd: session.cwd,
          updatedAt: session.lastMessageAt ?? "",
          lastMessageAt: session.lastMessageAt ?? null,
          archived: false,
          revision: session.threadRevision,
          lifecycle: session.running ? "running" as const : "ready" as const,
        })),
    ];
    const seen = seenMessageTimes.current;
    return summaries.map((summary) => {
      const session = agentState.sessions[summary.threadId];
      const lastMessageAt = newestMessageAt(summary.lastMessageAt, session?.lastMessageAt);
      const messageTime = lastMessageAt ? Date.parse(lastMessageAt) : 0;
      if (!seen.has(summary.threadId) || (surface === "thread" && summary.threadId === activeThreadId)) {
        seen.set(summary.threadId, Math.max(seen.get(summary.threadId) ?? 0, messageTime));
      }
      const working = (session?.running ?? false)
        || !["ready", "closed"].includes(summary.lifecycle ?? "ready");
      // Renames, settings, and other metadata changes are not unread messages.
      const unread = !working && messageTime > (seen.get(summary.threadId) ?? messageTime);
      return {
        id: summary.threadId,
        title: summary.title?.trim() || "New thread",
        folderId: collectionAssignments[summary.threadId] ?? null,
        updatedAt: lastMessageAt ? relativeTime(lastMessageAt) : "No messages yet",
        lastMessageAt,
        status: (working ? "working" : unread ? "unread" : "idle") as Thread["status"],
        messages: [],
      };
    }).sort(compareThreadMessages);
  }, [agentState.catalog, agentState.sessions, collectionAssignments, activeThreadId, surface]);

  const folders = useMemo(() => orderFoldersByMessages(unsortedFolders, threads), [unsortedFolders, threads]);

  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;
  const activeSession = activeThreadId ? agentState.sessions[activeThreadId] ?? null : null;
  const agentAccountScope = JSON.stringify([billingAccountId, agentState.runtimeInstanceId]);
  const newAgentSelection = newAgentDraft?.scope === agentAccountScope ? newAgentDraft.selection : DEFAULT_AGENT_SELECTION;
  const currentAgent = surface === "thread" ? activeSession?.desktopAgent : null;
  const agentControl: AgentControlOptions = {
    scope: JSON.stringify([agentAccountScope, surface === "thread" ? activeThreadId : "new"]),
    selection: currentAgent ? { enabled: currentAgent.enabled, permission: currentAgent.permission,
      workingDirectory: currentAgent.usesDefaultDirectory ? null : currentAgent.workingDirectory } : surface === "thread" ? DEFAULT_AGENT_SELECTION : newAgentSelection,
    defaultDirectory: currentAgent?.defaultWorkingDirectory,
    disabledReason: !billingAccountId || !agentState.connected ? "Sign in and connect to configure Agent mode."
      : typeof window.axiomDesktop?.agent.configureDesktopAgent !== "function" ? "Restart Axiom to enable the Agent controls."
      : surface === "thread" && !currentAgent ? "Agent settings are unavailable. Restart Axiom to load the updated native app."
      : agentSettingsBusy ? "Saving Agent settings…"
      : activeSession?.running || messageQueue.current!.list(activeThreadId, billingAccountId).length > 0 ? "Finish the current reply and send or remove queued messages before changing Agent settings."
      : undefined,
    onChooseDirectory: async (current) => {
      const api = window.axiomDesktop?.agent;
      if (!api?.chooseWorkingDirectory) throw new Error("Restart Axiom to enable the directory picker.");
      return api.chooseWorkingDirectory(current);
    },
    onApply: async (selection) => {
      if (agentControl.disabledReason) throw new Error(agentControl.disabledReason);
      const api = window.axiomDesktop!.agent;
      setAgentSettingsBusy(true);
      try {
        const current = await api.getState();
        if (!current.connected || current.account?.state !== "valid" || current.account.account?.id !== billingAccountId
          || current.runtimeInstanceId !== agentState.runtimeInstanceId) throw new Error("The account or connection changed. No Agent settings were applied.");
        if (surface === "thread" && activeThreadId && currentAgent) {
          await api.configureDesktopAgent({ threadId: activeThreadId, expectedRevision: currentAgent.revision, ...selection });
        } else {
          setNewAgentDraft({ scope: agentAccountScope, selection });
        }
      } finally { setAgentSettingsBusy(false); }
    },
  };
  const isStreaming = activeSession?.running ?? false;
  const accountView = accountPresentation(agentState.connected, desktopReady, agentState.account);
  const requiresSignIn = accountView.kind === "signed-out" || accountView.kind === "expired";

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    if (!api) {
      setUiError("Axiom Desktop bridge is unavailable");
      return;
    }
    let disposed = false;
    let receivedLiveState = false;
    const unsubscribe = api.onState((state) => {
      if (disposed) return;
      receivedLiveState = true;
      setAgentState(state);
    });
    void api.getState().then((state) => {
      // Subscription is installed first so an older IPC snapshot can never
      // overwrite a newer sidecar-ready event that raced its round trip.
      if (!disposed && !receivedLiveState) setAgentState(state);
    }).catch((error: unknown) => {
      if (!disposed) setUiError(String(error));
    });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, []);

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
        const rememberedModel = bootstrap.newThreadSettings.model ?? "";
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

  const refreshAccount = useCallback(async () => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected) return;
    try {
      await api.accountStatus();
      setModelCatalogRefresh((version) => version + 1);
      setUiError(null);
    } catch (error) {
      setUiError(error instanceof Error ? error.message : String(error));
    }
  }, [agentState.connected]);

  const refreshBilling = useCallback(async () => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected || agentState.account?.state !== "valid") {
      throw new Error("Sign in to load billing");
    }
    await api.billingStatus();
  }, [agentState.account?.state, agentState.connected]);

  const topUpBalance = useCallback(async (amountUsdCents: number) => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected || !billingAccountId) {
      throw new Error("Sign in to top up your balance");
    }
    const checkout = billingCheckout.current;
    if (!checkout) throw new Error("Checkout is unavailable");
    await billingMonitor.current?.stop();
    try {
      await checkout.topUp(billingAccountId, amountUsdCents, api);
    } finally {
      setBillingMonitorRevision((revision) => revision + 1);
    }
  }, [agentState.connected, billingAccountId]);

  const refreshBillingInvoice = useCallback(async (invoiceId: string) => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected || agentState.account?.state !== "valid") {
      throw new Error("Sign in to refresh an invoice");
    }
    await api.billingInvoiceStatus(invoiceId);
  }, [agentState.account?.state, agentState.connected]);

  const cancelNativeSignIn = useCallback(() => {
    signInGeneration.current += 1;
    signInPending.current = false;
    if (signInCloseTimer.current) {
      clearTimeout(signInCloseTimer.current);
      signInCloseTimer.current = null;
    }
    const loginId = activeLoginId.current;
    activeLoginId.current = null;
    setSignInState({ kind: "closed" });
    if (loginId) {
      void window.axiomDesktop?.agent.nativeLoginCancel(loginId).catch(() => undefined);
    }
  }, []);

  const openSignIn = useCallback(() => {
    setAccountScreen(null);
    setUiError(null);
    if (signInPending.current) return;
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected) {
      setSignInState({
        kind: "error",
        message: "AxiomCLI is not connected yet. Wait a moment and try again.",
      });
      return;
    }

    const generation = ++signInGeneration.current;
    signInPending.current = true;
    if (signInCloseTimer.current) {
      clearTimeout(signInCloseTimer.current);
      signInCloseTimer.current = null;
    }
    activeLoginId.current = null;
    setSignInState({ kind: "starting" });

    void (async () => {
      try {
        const started = await api.nativeLoginStart();
        const loginId = started.login.loginId;
        if (generation !== signInGeneration.current) {
          // A cancelled start can still finish its ACP round trip. Explicitly cancel
          // the short-lived server authorization without trying to complete it.
          void api.nativeLoginCancel(loginId).catch(() => undefined);
          return;
        }

        activeLoginId.current = loginId;
        setSignInState({ kind: "waiting", login: started.login });
        const completed = await api.nativeLoginComplete(loginId);
        if (activeLoginId.current === loginId) activeLoginId.current = null;
        if (generation !== signInGeneration.current) return;
        if (completed.status.state !== "valid") {
          throw new Error(completed.status.detail || "Axiom did not return a valid account after approval.");
        }

        signInPending.current = false;
        setSignInState({ kind: "success" });
        setModelCatalogRefresh((version) => version + 1);
        setUiError(null);
        signInCloseTimer.current = setTimeout(() => {
          if (generation === signInGeneration.current) setSignInState({ kind: "closed" });
          signInCloseTimer.current = null;
        }, 900);
      } catch (error) {
        if (generation !== signInGeneration.current) return;
        activeLoginId.current = null;
        signInPending.current = false;
        setSignInState({ kind: "error", message: signInErrorMessage(error) });
      }
    })();
  }, [agentState.connected]);

  useEffect(() => () => {
    signInGeneration.current += 1;
    if (signInCloseTimer.current) clearTimeout(signInCloseTimer.current);
    const loginId = activeLoginId.current;
    if (loginId) void window.axiomDesktop?.agent.nativeLoginCancel(loginId).catch(() => undefined);
  }, []);

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected || !desktopReady || agentState.account?.state !== "valid") {
      setModels([]);
      setModelCatalogError(null);
      return;
    }
    let cancelled = false;
    setModels([]);
    setModelCatalogError(null);
    void api.listModels().then((response) => {
      if (cancelled) return;
      const available = providerModels(response);
      if (available.length === 0) throw new Error("this account has no available models");
      const rememberedModel = preferredModelId.current;
      const nextModel = available.some((model) => model.id === rememberedModel)
        ? rememberedModel
        : available[0]!.id;
      setModels(available);
      if (!activeThreadIdRef.current) {
        const thinking = reconcileReasoningEffort(preferredReasoningEffort.current, available.find((model) => model.id === nextModel)!.supportedReasoningEfforts);
        setModelId(nextModel);
        setReasoningEffort(thinking);
        preferredModelId.current = nextModel;
        preferredReasoningEffort.current = thinking;
      }
    }).catch((error: unknown) => {
      if (cancelled) return;
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
    threadLoads.current.reset();
    setPendingUserMessage(null);
    setThreadLoadError(null);
    setDeleteTarget(null);
    setRenameTarget(null);
  }, [agentState.runtimeInstanceId, billingAccountId, agentState.connected]);

  useEffect(() => {
    if (!threadPreparation || (agentState.connected && agentState.account?.state === "valid"
      && threadPreparation.accountId === billingAccountId && threadPreparation.runtimeId === agentState.runtimeInstanceId)) return;
    cancelThreadPreparation();
    setSurface("welcome");
    if (threadPreparation.accountId !== billingAccountId) setDraft("");
    setUiError("The account or connection changed. Nothing was sent.");
  }, [threadPreparation, billingAccountId, agentState.connected, agentState.account?.state, agentState.runtimeInstanceId, cancelThreadPreparation, setDraft]);

  useEffect(() => () => { submittingDraft.current = null; }, []);

  const ensureThreadLoaded = useCallback(async (id: string, retry = false) => {
    const api = window.axiomDesktop?.agent;
    if (!api || activeThreadIdRef.current !== id || agentState.sessions[id]
      || threadLoads.current.isBlocked(id)) return;
    if (retry) setThreadLoadError(null);
    const result = await threadLoads.current.load(id, () => api.loadChat(id), retry);
    // Late load failures must not replace the welcome screen or a different
    // selection, especially after the thread has been deleted.
    if (activeThreadIdRef.current !== id || threadLoads.current.isBlocked(id)) return;
    if (result.state === "failed") {
      setThreadLoadError({ id, detail: desktopErrorMessage(result.error, "This thread could not be opened.") });
    } else if (result.state === "loaded") {
      setThreadLoadError(null);
    }
  }, [agentState.sessions]);

  const loadThread = useCallback(async (id: string) => {
    if (threadLoads.current.isBlocked(id)) return;
    cancelThreadPreparation();
    activeThreadIdRef.current = id;
    setActiveThreadId(id);
    setSurface("thread");
    setDraft("");
    setUiError(null);
    setThreadLoadError(null);
    await ensureThreadLoaded(id, true);
  }, [ensureThreadLoaded, cancelThreadPreparation, setDraft]);

  useEffect(() => {
    if (agentState.connected && desktopReady && surface === "thread" && activeThread
      && activeThreadId && !agentState.sessions[activeThreadId]) {
      // Reconciliation must never select a thread. A failed load is retried
      // only by an explicit click, not by every incoming state notification.
      void ensureThreadLoaded(activeThreadId);
    }
  }, [activeThreadId, activeThread, surface, desktopReady, agentState.connected, agentState.sessions, ensureThreadLoaded]);

  useEffect(() => {
    if (!activeSession?.settings) return;
    setModelId(activeSession.settings.model);
    const level = activeSession.settings.thinkingLevel;
    setReasoningEffort(isReasoningEffort(level) ? level : "provider_default");
  }, [activeSession?.settings]);

  useEffect(() => {
    settingsGeneration.current += 1;
  }, [activeThreadId]);

  useEffect(() => {
    if (!pendingUserMessage) return;
    const session = agentState.sessions[pendingUserMessage.sessionId];
    if (!session) return;
    const queued = messageQueue.current!.list(pendingUserMessage.sessionId, billingAccountId).find((item) => item.id === pendingUserMessage.id);
    if (session.timeline.some((item) => item.clientItemId === pendingUserMessage.id) || !queued || queued.status !== "sending") {
      setPendingUserMessage(null);
    }
  }, [agentState.sessions, pendingUserMessage, queueRevision, billingAccountId]);

  const showWelcome = useCallback(() => {
    cancelThreadPreparation();
    resetNewWeb();
    setNewAgentDraft(null);
    activeThreadIdRef.current = null;
    setSurface("welcome");
    setActiveThreadId(null);
    setDraft("");
    setThreadLoadError(null);
    setUiError(null);
    setModelId(preferredModelId.current);
    setReasoningEffort(preferredReasoningEffort.current);
  }, [resetNewWeb, cancelThreadPreparation, setDraft]);

  useEffect(() => {
    if (agentState.connected && desktopReady && surface === "thread" && activeThreadId
      && !activeThread && pendingUserMessage?.sessionId !== activeThreadId) {
      showWelcome();
    }
  }, [agentState.connected, desktopReady, surface, activeThreadId, activeThread, pendingUserMessage, showWelcome]);

  const showProxy = useCallback(() => {
    cancelThreadPreparation();
    setProxyInspecting(false);
    setSurface("proxy");
  }, [cancelThreadPreparation]);

  useEffect(() => {
    window.__axiomDemo = {
      setTheme: (theme) => setThemePreference(theme, { persist: false }),
      setSidebar: setSidebarOpen,
      showSettings: (open) => setAccountScreen(open ? "settings" : null),
      showWelcome,
      openModelPicker: () => {
        document.querySelector<HTMLButtonElement>('[title="Select model"]')?.click();
      },
      showProxy,
      showFolder: () => {
        for (const folder of folders) {
          if (folder.collapsed) void window.axiomDesktop?.agent.setCollectionCollapsed(folder.id, false);
        }
        const first = threads[0];
        if (first) void loadThread(first.id);
      },
      showThread: (id) => {
        const target = id && threads.some((thread) => thread.id === id) ? id : threads[0]?.id;
        if (target) void loadThread(target);
      },
      showSignIn: (open = true) => {
        document.querySelector<HTMLButtonElement>('[title="Select model"][aria-expanded="true"]')?.click();
        setAccountScreen(null);
        if (!open) {
          setSignInState({ kind: "closed" });
          return;
        }
        setSignInState({
          kind: "waiting",
          login: {
            loginId: "desktop-preview",
            userCode: "AXM7-K9Q2",
            authorizationUrl: "https://auth.axiom.stream/native/authorize",
            browserOpened: true,
            expiresAt: new Date(Date.now() + 10 * 60_000).toISOString(),
          },
        });
      },
    };
    return () => { delete window.__axiomDemo; };
  }, [folders, loadThread, showProxy, showWelcome, threads]);

  const submitDraft = async () => {
    const text = draft.trim();
    const api = window.axiomDesktop?.agent;
    if (!text || !api || !agentState.connected || submittingDraft.current || agentSettingsBusy) return;
    if (requiresSignIn) {
      openSignIn();
      return;
    }
    if (typeof api.promptWithWebConsent !== "function") {
      setUiError("Restart Axiom to apply the Web privacy controls. No message was sent.");
      return;
    }
    if (!selectedModel) {
      setUiError("Wait for the model catalog before sending.");
      return;
    }
    const webEnabled = webAccess.enabled;
    const creatingThread = surface !== "thread" || !activeThreadId;
    const submissionId = crypto.randomUUID();
    submittingDraft.current = submissionId;
    preferredModelId.current = selectedModel.id;
    preferredReasoningEffort.current = effectiveReasoningEffort;
    setUiError(null);
    if (creatingThread) {
      // Render locally before the first IPC round trip. Keep the draft and
      // Agent choices until setup and durable queueing have both succeeded.
      setThreadPreparation({ id: submissionId, text, accountId: billingAccountId, runtimeId: agentState.runtimeInstanceId });
      activeThreadIdRef.current = null;
      setActiveThreadId(null);
      setThreadLoadError(null);
      setSurface("thread");
    }
    const currentSubmissionState = async () => {
      if (submittingDraft.current !== submissionId) return null;
      const current = await api.getState();
      if (submittingDraft.current !== submissionId) return null;
      if (!current.connected || current.account?.state !== "valid" || current.account.account?.id !== billingAccountId
        || current.runtimeInstanceId !== agentState.runtimeInstanceId) {
        throw new Error("The account or connection changed. Nothing was sent.");
      }
      return current;
    };
    try {
      let sessionId = creatingThread ? null : activeThreadId;
      if (creatingThread) {
        const created = await api.newChat();
        if (!await currentSubmissionState()) return;
        sessionId = created.sessionId;
        await api.setSettings({ threadId: sessionId, model: selectedModel.id, thinkingLevel: effectiveReasoningEffort });
        if (!await currentSubmissionState()) return;
        if (newAgentSelection.enabled || newAgentSelection.permission !== "approve_commands" || newAgentSelection.workingDirectory) {
          if (typeof api.configureDesktopAgent !== "function") throw new Error("Restart Axiom to enable Agent mode. Your draft was kept.");
          await api.configureDesktopAgent({ threadId: sessionId, expectedRevision: 0, ...newAgentSelection });
        }
      } else if (sessionId && !agentState.sessions[sessionId]) {
        await api.loadChat(sessionId);
      }
      if (!sessionId) throw new Error("The thread could not be created. Your draft was kept.");
      const loaded = await currentSubmissionState();
      if (!loaded) return;
      const settings = loaded.sessions[sessionId]?.settings;
      if (!loaded.sessions[sessionId]?.running && settings && (settings.model !== selectedModel.id || settings.thinkingLevel !== effectiveReasoningEffort)) {
        await api.setSettings({ threadId: sessionId, model: selectedModel.id, thinkingLevel: effectiveReasoningEffort });
      }
      const current = await currentSubmissionState();
      if (!current) return;
      if (creatingThread) webAccess.adoptThread(sessionId);
      messageQueue.current!.sync(current, api);
      const messageId = messageQueue.current!.enqueue(sessionId, text, webEnabled);
      if (creatingThread) {
        activeThreadIdRef.current = sessionId;
        setActiveThreadId(sessionId);
        setPendingUserMessage({ id: messageId, sessionId, text });
        setNewAgentDraft(null);
        setThreadPreparation(null);
      }
      setDraft((current) => current.trim() === text ? "" : current);
      setUiError(null);
    } catch (error) {
      // A late completion must never reopen a thread, restore an old draft, or
      // overwrite the error belonging to a newer submission/account.
      if (submittingDraft.current !== submissionId) return;
      if (creatingThread) {
        setThreadPreparation(null);
        setSurface("welcome");
      }
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      if (submittingDraft.current === submissionId) submittingDraft.current = null;
    }
  };

  const stopStream = useCallback(() => {
    if (activeThreadId) {
      messageQueue.current?.pause(activeThreadId);
      void window.axiomDesktop?.agent.cancel(activeThreadId);
    }
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

  const deleteThread = async (id: string) => {
    const api = window.axiomDesktop?.agent;
    if (!api || threadLoads.current.isBlocked(id)) return;
    // Invalidate selection and pending loads before the first IPC await: the
    // sidecar publishes removal before deleteConfirm finishes its refresh.
    threadLoads.current.block(id);
    if (activeThreadIdRef.current === id) showWelcome();
    setPendingUserMessage((current) => current?.sessionId === id ? null : current);
    try {
      const preview = await api.deletePreview([id]);
      await api.deleteConfirm(preview.confirmationToken, [id]);
      webAccess.forgetThread(id);
      messageQueue.current?.forget(id);
    } catch (error) {
      threadLoads.current.unblock(id);
      setUiError(desktopErrorMessage(error, "This thread could not be deleted."));
    }
  };

  const title = surface === "proxy"
    ? (proxyInspecting ? "Attestation" : "Proxy")
    : surface === "welcome" || !activeThread
      ? "New thread"
      : activeThread.title;

  const modelStatusLabel = !agentState.connected || !desktopReady
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

  const runCollectionIntent = (intent: Promise<unknown> | undefined) => {
    void intent?.catch((error: unknown) => {
      setUiError(error instanceof Error ? error.message : String(error));
    });
  };

  const macNotch = isMac && !sidebarOpen && !fullScreen;
  const windowControlsCutout = !isMac && !fullScreen;

  return (
    <div className="app-canvas flex h-full overflow-hidden p-2.5 text-[var(--color-text-primary)]">
      <TrafficLightGhost />
      {windowControlsCutout ? <WindowControls /> : null}
      {/* The canvas gutter above the stage card is still window chrome. */}
      <div
        className={`drag-region fixed left-0 top-0 z-40 h-2.5 ${windowControlsCutout ? "right-[112px]" : "right-0"}`}
        aria-hidden="true"
      />
      {/* In-flow column that collapses via the margin trick; stays mounted so
          the 300ms slide can play both ways. */}
      <div
        className="relative flex h-full w-[260px] shrink-0 flex-col transition-[margin-left,transform] duration-300 ease-[var(--ease-out-expo)]"
        // −260 keeps the card in the 10px gutter; the extra −10px transform takes
        // the wrapper (and its scrollbar) fully off-canvas without moving layout.
        style={{ marginLeft: sidebarOpen ? 0 : -SIDEBAR_WIDTH, transform: sidebarOpen ? "none" : "translateX(-10px)" }}
        inert={!sidebarOpen || accountScreenOpen}
        aria-hidden={!sidebarOpen || accountScreenOpen}
      >
          <Sidebar
            folders={folders}
            threads={threads}
            activeThreadId={surface === "thread" ? activeThreadId : null}
            proxyActive={surface === "proxy"}
            welcomeActive={surface === "welcome"}
            onCollapse={() => setSidebarOpen(false)}
            onNewChat={showWelcome}
            onOpenProxy={showProxy}
            onSelectThread={(id) => void loadThread(id)}
            onCreateFolder={() => runCollectionIntent(
              window.axiomDesktop?.agent.createCollection("Untitled folder"),
            )}
            onToggleFolder={(id) => {
              const folder = folders.find((candidate) => candidate.id === id);
              if (folder) runCollectionIntent(
                window.axiomDesktop?.agent.setCollectionCollapsed(id, !folder.collapsed),
              );
            }}
            onRenameFolder={(id) => {
              const folder = folders.find((candidate) => candidate.id === id);
              if (folder) setRenameTarget({ kind: "folder", id, title: folder.name });
            }}
            onRenameThread={(id) => {
              const thread = threads.find((candidate) => candidate.id === id);
              if (thread && !threadLoads.current.isBlocked(id)) setRenameTarget({ kind: "thread", id, title: thread.title });
            }}
            onDeleteFolder={(id) => {
              const folder = folders.find((candidate) => candidate.id === id);
              if (folder) setDeleteTarget({ kind: "folder", id, title: folder.name });
            }}
            onDeleteThread={(id) => {
              const thread = threads.find((candidate) => candidate.id === id);
              if (thread && !threadLoads.current.isBlocked(id)) {
                setDeleteTarget({ kind: "thread", id, title: thread.title });
              }
            }}
            onMoveThread={(threadId, folderId) => runCollectionIntent((async () => {
              if (folderId && folders.find((folder) => folder.id === folderId)?.collapsed) {
                await window.axiomDesktop?.agent.setCollectionCollapsed(folderId, false);
              }
              await window.axiomDesktop?.agent.assignThreadCollection(threadId, folderId);
            })())}
            account={accountView}
            onSignIn={openSignIn}
            connected={agentState.connected}
            accountId={billingAccountId}
            billing={agentState.billing}
            onRefreshBalance={refreshBilling}
            onOpenSettings={(trigger) => {
              accountScreenReturnFocus.current = trigger;
              setSettingsCategory("appearance");
              setAccountScreen("settings");
            }}
            onOpenBalance={(trigger) => {
              accountScreenReturnFocus.current = trigger;
              setAccountScreen("balance");
            }}
          />
      </div>

      <div
        className="stage-frame relative min-w-0 flex-1"
        inert={accountScreenOpen}
        aria-hidden={accountScreenOpen}
        data-notch={macNotch}
        data-controls-cutout={windowControlsCutout}
      >
        <div className="stage-shadow pointer-events-none absolute inset-0 rounded-[22px]" aria-hidden="true" />
        <div className="stage-ring pointer-events-none absolute inset-0" aria-hidden="true" />
        <section className="stage-card absolute inset-px flex flex-col overflow-hidden rounded-[21px] bg-[var(--surface-stage)]">
        <header
          className={[
            "drag-region absolute left-0 top-0 z-30 flex h-11 items-center bg-transparent",
            windowControlsCutout ? "right-[112px]" : "right-0",
            macNotch ? MAC_TRAFFIC_LIGHT_INSET_CLASS : "pl-3",
            "pr-2",
          ].join(" ")}
        >
          {!sidebarOpen ? <button type="button" onClick={() => setSidebarOpen(true)} className="no-drag mr-2 rounded-md p-1.5 text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-surface-hover)]" aria-label="Open sidebar"><PanelLeftOpen size={16} /></button> : null}
          <div className="flex min-w-0 flex-1 items-center gap-2 pl-1 text-[13px] font-medium tracking-tight">
            <OverflowTitle text={title} />
            {surface === "thread" && activeThread?.folderId ? (() => {
              const folder = folders.find((entry) => entry.id === activeThread.folderId);
              return folder ? (
                <span className="pointer-events-none inline-flex shrink-0 items-center gap-1 rounded-full bg-[var(--wash-chip)] px-2 py-0.5 text-[11px] font-normal text-[var(--color-text-tertiary)]">
                  <Folder size={10} />
                  <span className="max-w-[10rem]"><OverflowTitle text={folder.name} /></span>
                </span>
              ) : null;
            })() : null}
          </div>
          <UpdateNotice state={updates.state} onOpen={(trigger) => {
            accountScreenReturnFocus.current = trigger;
            setSettingsCategory("updates");
            setAccountScreen("settings");
          }} />
        </header>

        {signInState.kind !== "closed" ? (
          <div className="mx-5 mb-2 mt-12 shrink-0">
            <SignInStatus state={signInState} onCancel={cancelNativeSignIn} onRetry={openSignIn} />
          </div>
        ) : null}

        {uiError || webAccess.error || agentState.error ? (
          <div className="shadow-glass-pop absolute left-1/2 top-12 z-40 max-w-[80%] -translate-x-1/2 truncate rounded-full bg-[var(--surface-float)] px-4 py-2 text-[12.5px] text-[var(--color-danger-strong)] backdrop-blur">{uiError ?? webAccess.error ?? agentState.error}</div>
        ) : modelCatalogError ? (
          <div className="shadow-glass-pop absolute left-1/2 top-12 z-40 max-w-[80%] -translate-x-1/2 truncate rounded-full bg-[var(--surface-float)] px-4 py-2 text-[12.5px] text-[var(--color-text-secondary)] backdrop-blur">{modelCatalogError}</div>
        ) : null}

        <div className="relative min-h-0 flex-1">
          {surface === "proxy" ? <ProxyView accountId={agentState.connected ? billingAccountId : null} runtimeId={agentState.runtimeInstanceId} onInspecting={setProxyInspecting} /> : surface === "thread" && threadPreparation && threadPreparation.accountId === billingAccountId && threadPreparation.runtimeId === agentState.runtimeInstanceId ? (
            <PreparingThreadView id={threadPreparation.id} text={threadPreparation.text} />
          ) : surface === "thread" && activeSession ? (
            <ChatView
              agentControl={agentControl}
              queue={<MessageQueue messages={messageQueue.current.list(activeThreadId, billingAccountId).filter((message) => message.id !== pendingUserMessage?.id || message.status !== "sending")}
                paused={!!activeThreadId && messageQueue.current.isPaused(activeThreadId)}
                canSteer={agentState.connected && (!activeSession.running || !!activeSession.activeTurnId)}
                onSendNow={(id) => messageQueue.current?.sendNow(id)} onRemove={(id) => messageQueue.current?.remove(id)} />}
              webEnabled={webAccess.enabled}
              webDisabled={!billingAccountId || !agentState.connected}
              onWebToggle={webAccess.toggle}
              session={activeSession}
              draft={draft}
              onDraftChange={setDraft}
              onSubmit={() => void submitDraft()}
              onStop={stopStream}
              isStreaming={isStreaming}
              models={models}
              selectedModelId={modelId || models[0]?.id || ""}
              modelStatusLabel={modelStatusLabel}
              onModelChange={(id) => updateSettings(id, undefined)}
              requiresSignIn={requiresSignIn}
              onSignIn={openSignIn}
              reasoningEffort={effectiveReasoningEffort}
              onReasoningEffortChange={(effort) => updateSettings(undefined, effort)}
              pendingUserMessage={pendingUserMessage}
              securityScope={agentAccountScope}
              securityRefreshDisabledReason={!agentState.connected ? "Reconnect to refresh verification" : requiresSignIn ? "Sign in to refresh verification" : undefined}
              onVerifySecurity={async () => {
                if (!activeThreadId || !window.axiomDesktop?.agent) throw new Error("No active thread");
                await window.axiomDesktop.agent.verifySecurity(activeThreadId);
              }}
            />
          ) : surface === "thread" ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-[13px] text-[var(--color-text-tertiary)]">
              {threadLoadError?.id === activeThreadId ? (
                <>
                  <p role="alert">Could not open this thread. {threadLoadError.detail}</p>
                  <div className="flex gap-3">
                    <button type="button" className="rounded-md bg-[var(--wash-row)] px-3 py-2" onClick={() => {
                      if (activeThreadId) void ensureThreadLoaded(activeThreadId, true);
                    }}>Retry</button>
                    <button type="button" className="rounded-md bg-[var(--wash-row)] px-3 py-2" onClick={showWelcome}>New thread</button>
                  </div>
                </>
              ) : "Loading thread…"}
            </div>
          ) : (
            <WelcomeView
              agentControl={agentControl}
              webEnabled={webAccess.enabled}
              webDisabled={!billingAccountId || !agentState.connected}
              onWebToggle={webAccess.toggle}
              draft={draft}
              onDraftChange={setDraft}
              onSubmit={() => void submitDraft()}
              models={models}
              selectedModelId={modelId || models[0]?.id || ""}
              modelStatusLabel={modelStatusLabel}
              onModelChange={(id) => {
                const selected = models.find((model) => model.id === id);
                if (!selected) return;
                const thinking = reconcileReasoningEffort(reasoningEffort, selected.supportedReasoningEfforts);
                setModelId(id); preferredModelId.current = id;
                setReasoningEffort(thinking); preferredReasoningEffort.current = thinking;
              }}
              account={accountView}
              signInActive={signInState.kind !== "closed"}
              onSignIn={openSignIn}
              onRefreshAccount={() => void refreshAccount()}
              reasoningEffort={effectiveReasoningEffort}
              onReasoningEffortChange={(effort) => {
                const thinking = reconcileReasoningEffort(effort, selectedModel?.supportedReasoningEfforts ?? []);
                setReasoningEffort(thinking);
                preferredReasoningEffort.current = thinking;
              }}
            />
          )}
        </div>
      </section>
      </div>
      {webAccess.warning ? <WebConsentDialog onCancel={webAccess.cancel} onConfirm={webAccess.confirm} /> : null}
      {renameTarget ? <RenameDialog
        key={`${renameTarget.kind}:${renameTarget.id}`}
        kind={renameTarget.kind}
        title={renameTarget.title}
        onCancel={() => setRenameTarget(null)}
        onSave={async (name) => {
          const api = window.axiomDesktop?.agent;
          if (!api) throw new Error("The desktop service is unavailable. Please try again.");
          return renameTarget.kind === "thread"
            ? api.renameThread(renameTarget.id, name)
            : api.renameCollection(renameTarget.id, name);
        }}
      /> : null}
      {deleteTarget ? (
        <DeleteConfirmationDialog
          kind={deleteTarget.kind}
          title={deleteTarget.title}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => {
            setDeleteTarget(null);
            if (deleteTarget.kind === "thread") void deleteThread(deleteTarget.id);
            else runCollectionIntent(window.axiomDesktop?.agent.deleteCollection(deleteTarget.id));
          }}
        />
      ) : null}
      {accountScreen === "settings" ? (
        <SettingsSheet
          initialCategory={settingsCategory}
          updates={updates}
          runtimeInstanceId={agentState.runtimeInstanceId}
          onClose={() => {
            setAccountScreen(null);
            const opener = accountScreenReturnFocus.current;
            accountScreenReturnFocus.current = null;
            requestAnimationFrame(() => { if (opener?.isConnected) opener.focus({ preventScroll: true }); });
          }}
          connected={agentState.connected}
          account={agentState.account}
          onRefreshAccount={() => {
            void refreshAccount();
          }}
          onLogin={openSignIn}
          onManageAccount={() => {
            void window.axiomDesktop?.agent.openAccountPortal().catch((error: unknown) => {
              setUiError(error instanceof Error ? error.message : String(error));
            });
          }}
          onLogout={() => {
            void (async () => {
              try {
                await window.axiomDesktop?.agent.logout();
                setModels([]);
                setModelCatalogError(null);
                setUiError(null);
              } catch (error) {
                setUiError(error instanceof Error ? error.message : String(error));
              }
            })();
          }}
        />
      ) : null}
      {accountScreen === "balance" ? (
        <BalanceScreen
          onClose={() => {
            setAccountScreen(null);
            const opener = accountScreenReturnFocus.current;
            accountScreenReturnFocus.current = null;
            requestAnimationFrame(() => { if (opener?.isConnected) opener.focus({ preventScroll: true }); });
          }}
          connected={agentState.connected}
          account={agentState.account}
          billing={agentState.billing}
          invoice={agentState.invoice}
          onRefreshBilling={refreshBilling}
          onTopUp={topUpBalance}
          onRefreshInvoice={refreshBillingInvoice}
          onLogin={openSignIn}
        />
      ) : null}
    </div>
  );
}

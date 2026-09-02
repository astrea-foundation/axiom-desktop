import { Folder, MoreHorizontal, PanelLeftOpen } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ClientState, ListModelsResponse } from "@axiom/axiom-acp-client";
import { ChatView } from "./components/ChatView";
import { ProxyView } from "./components/ProxyView";
import { SettingsSheet } from "./components/SettingsSheet";
import { SignInDialog } from "./components/SignInDialog";
import { Sidebar } from "./components/Sidebar";
import { WelcomeView } from "./components/WelcomeView";
import { TrafficLightGhost } from "./components/TrafficLightGhost";
import { WindowControls } from "./components/WindowControls";
import { setThemePreference, useTheme } from "./lib/theme";
import { isMac, MAC_TRAFFIC_LIGHT_INSET_CLASS, useFullScreen } from "./lib/window-chrome";
import { accountPresentation, desktopErrorMessage, signInErrorMessage, type SignInState } from "./signInFlow";
import type { MainSurface, PendingUserMessage, ProviderModel, ReasoningEffort, Thread } from "./types";

const SIDEBAR_WIDTH = 260;
const REASONING_EFFORTS: readonly ReasoningEffort[] = ["minimal", "low", "medium", "high", "xhigh"];

function isReasoningEffort(value: string | undefined): value is ReasoningEffort {
  return REASONING_EFFORTS.includes(value as ReasoningEffort);
}

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
      supportedReasoningEfforts: model.thinkingLevels.filter(isReasoningEffort),
      inputPriceMicrousdPerMillionTokens: inputPrice,
      outputPriceMicrousdPerMillionTokens: outputPrice,
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
  const [agentState, setAgentState] = useState<ClientState>(EMPTY_AGENT_STATE);
  const [models, setModels] = useState<ProviderModel[]>([]);
  const [surface, setSurface] = useState<MainSurface>("welcome");
  const [activeThreadId, setActiveThreadId] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const fullScreen = useFullScreen();
  useTheme();
  const [draft, setDraft] = useState("");
  const [modelId, setModelId] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("medium");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [proxyInspecting, setProxyInspecting] = useState(false);
  const [uiError, setUiError] = useState<string | null>(null);
  const [modelCatalogError, setModelCatalogError] = useState<string | null>(null);
  const [modelCatalogRefresh, setModelCatalogRefresh] = useState(0);
  const [desktopReady, setDesktopReady] = useState(false);
  const [pendingUserMessage, setPendingUserMessage] = useState<PendingUserMessage | null>(null);
  const [signInState, setSignInState] = useState<SignInState>({ kind: "closed" });
  const loadingSessions = useRef(new Set<string>());
  const seenRevisions = useRef(new Map<string, number>());
  const settingsGeneration = useRef(0);
  const preferredModelId = useRef("");
  const preferredReasoningEffort = useRef<ReasoningEffort>("medium");
  const signInGeneration = useRef(0);
  const activeLoginId = useRef<string | null>(null);
  const signInCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const folders = useMemo(() => agentState.collections.collections.map((collection) => ({
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
          updatedAt: new Date().toISOString(),
          archived: false,
          revision: session.threadRevision,
          lifecycle: session.running ? "running" as const : "ready" as const,
        })),
    ];
    const seen = seenRevisions.current;
    return summaries.map((summary) => {
      const session = agentState.sessions[summary.threadId];
      const revision = Math.max(summary.revision ?? 0, session?.threadRevision ?? 0);
      if (!seen.has(summary.threadId) || summary.threadId === activeThreadId) {
        seen.set(summary.threadId, revision);
      }
      const working = (session?.running ?? false)
        || !["ready", "closed"].includes(summary.lifecycle ?? "ready");
      const unread = !working && revision > (seen.get(summary.threadId) ?? revision);
      return {
        id: summary.threadId,
        title: summary.title?.trim() || "New thread",
        folderId: collectionAssignments[summary.threadId] ?? null,
        updatedAt: relativeTime(summary.updatedAt),
        status: (working ? "working" : unread ? "unread" : "idle") as Thread["status"],
        messages: [],
      };
    });
  }, [agentState.catalog, agentState.sessions, collectionAssignments, activeThreadId]);

  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;
  const activeSession = activeThreadId ? agentState.sessions[activeThreadId] ?? null : null;
  const isStreaming = activeSession?.running ?? false;
  const accountView = accountPresentation(agentState.connected, desktopReady, agentState.account);
  const requiresSignIn = accountView.kind === "signed-out" || accountView.kind === "invalid";

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
        const nextThinking = isReasoningEffort(rememberedThinking) ? rememberedThinking : "medium";
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

  const cancelBrowserSignIn = useCallback(() => {
    signInGeneration.current += 1;
    if (signInCloseTimer.current) {
      clearTimeout(signInCloseTimer.current);
      signInCloseTimer.current = null;
    }
    const loginId = activeLoginId.current;
    activeLoginId.current = null;
    setSignInState({ kind: "closed" });
    if (loginId) {
      void window.axiomDesktop?.agent.browserLoginCancel(loginId).catch(() => undefined);
    }
  }, []);

  const beginBrowserSignIn = useCallback(() => {
    const api = window.axiomDesktop?.agent;
    if (!api || !agentState.connected) {
      setSignInState({
        kind: "error",
        message: "AxiomCLI is not connected yet. Wait a moment and try again.",
      });
      return;
    }

    const generation = ++signInGeneration.current;
    if (signInCloseTimer.current) {
      clearTimeout(signInCloseTimer.current);
      signInCloseTimer.current = null;
    }
    activeLoginId.current = null;
    setSettingsOpen(false);
    setUiError(null);
    setSignInState({ kind: "starting" });

    void (async () => {
      try {
        const started = await api.browserLoginStart();
        const loginId = started.login.loginId;
        if (generation !== signInGeneration.current) {
          // A cancelled start can still finish its ACP round trip. Completing and
          // immediately cancelling it removes the short-lived login from AxiomCLI.
          void api.browserLoginComplete(loginId).catch(() => undefined);
          void api.browserLoginCancel(loginId).catch(() => undefined);
          return;
        }

        activeLoginId.current = loginId;
        setSignInState({ kind: "waiting", login: started.login });
        const completed = await api.browserLoginComplete(loginId);
        if (activeLoginId.current === loginId) activeLoginId.current = null;
        if (generation !== signInGeneration.current) return;
        if (completed.status.state !== "valid") {
          throw new Error(completed.status.detail || "Axiom did not return a valid account after approval.");
        }

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
        setSignInState({ kind: "error", message: signInErrorMessage(error) });
      }
    })();
  }, [agentState.connected]);

  useEffect(() => () => {
    signInGeneration.current += 1;
    if (signInCloseTimer.current) clearTimeout(signInCloseTimer.current);
    const loginId = activeLoginId.current;
    if (loginId) void window.axiomDesktop?.agent.browserLoginCancel(loginId).catch(() => undefined);
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
      setModelId(nextModel);
      preferredModelId.current = nextModel;
    }).catch((error: unknown) => {
      if (cancelled) return;
      const detail = desktopErrorMessage(error, "AxiomCLI could not load the model catalog.");
      setModelCatalogError(`Could not load models. Check your connection or refresh your account in Settings. ${detail}`);
    });
    return () => { cancelled = true; };
  }, [
    agentState.account?.keyId,
    agentState.account?.state,
    agentState.connected,
    agentState.runtimeInstanceId,
    desktopReady,
    modelCatalogRefresh,
  ]);

  const loadThread = useCallback(async (id: string) => {
    setActiveThreadId(id);
    setSurface("thread");
    setDraft("");
    if (agentState.sessions[id] || loadingSessions.current.has(id)) return;
    loadingSessions.current.add(id);
    try {
      await window.axiomDesktop?.agent.loadChat(id);
      setUiError(null);
    } catch (error) {
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      loadingSessions.current.delete(id);
    }
  }, [agentState.sessions]);

  useEffect(() => {
    if (agentState.connected && activeThreadId && !agentState.sessions[activeThreadId]) {
      void loadThread(activeThreadId);
    }
  }, [activeThreadId, agentState.connected, agentState.sessions, loadThread]);

  useEffect(() => {
    if (!activeSession?.settings) return;
    setModelId(activeSession.settings.model);
    const level = activeSession.settings.thinkingLevel;
    setReasoningEffort(isReasoningEffort(level) ? level : "medium");
  }, [activeSession?.settings]);

  useEffect(() => {
    settingsGeneration.current += 1;
  }, [activeThreadId]);

  useEffect(() => {
    if (!pendingUserMessage) return;
    const session = agentState.sessions[pendingUserMessage.sessionId];
    if (!session) return;
    if (session.timeline.some((item) => item.clientItemId === pendingUserMessage.id)) {
      setPendingUserMessage(null);
    }
  }, [agentState.sessions, pendingUserMessage]);

  const showWelcome = useCallback(() => {
    setSurface("welcome");
    setActiveThreadId(null);
    setDraft("");
    setModelId(preferredModelId.current);
    setReasoningEffort(preferredReasoningEffort.current);
  }, []);

  const showProxy = useCallback(() => {
    setProxyInspecting(false);
    setSurface("proxy");
  }, []);

  useEffect(() => {
    window.__axiomDemo = {
      setTheme: (theme) => setThemePreference(theme, { persist: false }),
      setSidebar: setSidebarOpen,
      showSettings: setSettingsOpen,
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
        setSettingsOpen(false);
        if (!open) {
          setSignInState({ kind: "closed" });
          return;
        }
        setSignInState({
          kind: "waiting",
          login: {
            loginId: "desktop-preview",
            userCode: "AXM7-K9Q2",
            authorizationUrl: "https://axiom.trade/cli/authorize",
            browserOpened: true,
          },
        });
      },
    };
    return () => { delete window.__axiomDemo; };
  }, [folders, loadThread, showProxy, showWelcome, threads]);

  const submitDraft = async () => {
    const text = draft.trim();
    const api = window.axiomDesktop?.agent;
    if (!text || !api || !agentState.connected) return;
    if (requiresSignIn) {
      beginBrowserSignIn();
      return;
    }
    const submissionId = crypto.randomUUID();
    const showPendingMessage = (sessionId: string) => {
      setPendingUserMessage({
        id: submissionId,
        sessionId,
        text,
      });
    };
    setDraft("");
    preferredModelId.current = modelId;
    preferredReasoningEffort.current = reasoningEffort;
    try {
      let sessionId = activeThreadId;
      if (surface !== "thread" || !sessionId) {
        const created = await api.newChat();
        sessionId = created.sessionId;
        setActiveThreadId(sessionId);
        setSurface("thread");
        showPendingMessage(sessionId);
        await api.setSettings({
          threadId: sessionId,
          ...(modelId ? { model: modelId } : {}),
          thinkingLevel: reasoningEffort,
        });
      } else if (!agentState.sessions[sessionId]) {
        await api.loadChat(sessionId);
        showPendingMessage(sessionId);
      } else {
        showPendingMessage(sessionId);
      }
      await api.prompt(sessionId, text, submissionId);
      await api.listThreads();
      setUiError(null);
    } catch (error) {
      setPendingUserMessage((current) => current?.id === submissionId ? null : current);
      setDraft((current) => current || text);
      setUiError(error instanceof Error ? error.message : String(error));
    }
  };

  const stopStream = useCallback(() => {
    if (activeThreadId) void window.axiomDesktop?.agent.cancel(activeThreadId);
  }, [activeThreadId]);

  const updateSettings = (nextModel?: string, nextThinking?: ReasoningEffort) => {
    if (!activeThreadId || !activeSession) return;
    const api = window.axiomDesktop?.agent;
    if (!api) return;
    const generation = ++settingsGeneration.current;
    if (nextModel) setModelId(nextModel);
    if (nextThinking) setReasoningEffort(nextThinking);
    void api.setSettings({
      threadId: activeThreadId,
      ...(nextModel ? { model: nextModel } : {}),
      ...(nextThinking ? { thinkingLevel: nextThinking } : {}),
    }).then((result) => {
      if (generation !== settingsGeneration.current || result.threadId !== activeThreadId) return;
      const level = result.settings.thinkingLevel;
      setModelId(result.settings.model);
      setReasoningEffort(isReasoningEffort(level) ? level : "medium");
      preferredModelId.current = result.settings.model;
      preferredReasoningEffort.current = isReasoningEffort(level) ? level : "medium";
      setUiError(null);
    }).catch((error: unknown) => {
      if (generation !== settingsGeneration.current) return;
      if (activeSession.settings) {
        setModelId(activeSession.settings.model);
        const level = activeSession.settings.thinkingLevel;
        setReasoningEffort(isReasoningEffort(level) ? level : "medium");
      }
      setUiError(error instanceof Error ? error.message : String(error));
    });
  };

  const deleteThread = async (id: string) => {
    if (!window.confirm("Delete this thread permanently?")) return;
    try {
      const preview = await window.axiomDesktop?.agent.deletePreview([id]);
      if (!preview) return;
      await window.axiomDesktop?.agent.deleteConfirm(preview.confirmationToken, [id]);
      if (activeThreadId === id) showWelcome();
    } catch (error) {
      setUiError(error instanceof Error ? error.message : String(error));
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
      : agentState.account?.state === "invalid"
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

  return (
    <div className="app-canvas flex h-full overflow-hidden p-2.5 text-[var(--color-text-primary)]">
      <TrafficLightGhost />
      {/* The canvas gutter above the stage card is still window chrome. */}
      <div className="drag-region fixed inset-x-0 top-0 z-40 h-2.5" aria-hidden="true" />
      {/* In-flow column that collapses via the margin trick; stays mounted so
          the 300ms slide can play both ways. */}
      <div
        className="relative flex h-full w-[260px] shrink-0 flex-col transition-[margin-left,transform] duration-300 ease-[var(--ease-out-expo)]"
        // −260 keeps the card in the 10px gutter; the extra −10px transform takes
        // the wrapper (and its scrollbar) fully off-canvas without moving layout.
        style={{ marginLeft: sidebarOpen ? 0 : -SIDEBAR_WIDTH, transform: sidebarOpen ? "none" : "translateX(-10px)" }}
        inert={!sidebarOpen}
        aria-hidden={!sidebarOpen}
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
            onRenameFolder={(id, name) => runCollectionIntent(
              window.axiomDesktop?.agent.renameCollection(id, name),
            )}
            onDeleteFolder={(id) => {
              if (window.confirm("Delete this folder? Its threads will remain available.")) {
                runCollectionIntent(window.axiomDesktop?.agent.deleteCollection(id));
              }
            }}
            onDeleteThread={(id) => void deleteThread(id)}
            onMoveThread={(threadId, folderId) => runCollectionIntent((async () => {
              if (folderId && folders.find((folder) => folder.id === folderId)?.collapsed) {
                await window.axiomDesktop?.agent.setCollectionCollapsed(folderId, false);
              }
              await window.axiomDesktop?.agent.assignThreadCollection(threadId, folderId);
            })())}
            account={accountView}
            onSignIn={beginBrowserSignIn}
            onOpenSettings={() => setSettingsOpen(true)}
          />
      </div>

      <div className="stage-frame relative min-w-0 flex-1" data-notch={macNotch}>
        <div className="stage-shadow pointer-events-none absolute inset-0 rounded-[22px]" aria-hidden="true" />
        <div className="stage-ring pointer-events-none absolute inset-0" aria-hidden="true" />
        <section className="stage-card absolute inset-px flex flex-col overflow-hidden rounded-[21px] bg-[var(--surface-stage)]">
        <header
          className={[
            "drag-region absolute inset-x-0 top-0 z-30 flex h-11 items-center bg-transparent",
            macNotch ? MAC_TRAFFIC_LIGHT_INSET_CLASS : "pl-3",
            "pr-2",
          ].join(" ")}
        >
          {!sidebarOpen ? <button type="button" onClick={() => setSidebarOpen(true)} className="no-drag mr-2 rounded-md p-1.5 text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-surface-hover)]" aria-label="Open sidebar"><PanelLeftOpen size={16} /></button> : null}
          <div className="flex min-w-0 flex-1 items-center gap-2 pl-1 text-[13px] font-medium tracking-tight">
            <span className="min-w-0 truncate">{title}</span>
            {surface === "thread" && activeThread?.folderId ? (() => {
              const folder = folders.find((entry) => entry.id === activeThread.folderId);
              return folder ? (
                <span className="pointer-events-none inline-flex shrink-0 items-center gap-1 rounded-full bg-[var(--wash-chip)] px-2 py-0.5 text-[11px] font-normal text-[var(--color-text-tertiary)]">
                  <Folder size={10} />
                  <span className="max-w-[10rem] truncate">{folder.name}</span>
                </span>
              ) : null;
            })() : null}
            {surface === "thread" ? (
              <button
                type="button"
                className="no-drag shrink-0 rounded-md p-1 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-hover-strong)] hover:text-[var(--color-text-secondary)]"
                aria-label="Thread actions"
              >
                <MoreHorizontal size={15} />
              </button>
            ) : null}
          </div>
          <WindowControls />
        </header>

        {uiError || agentState.error ? (
          <div className="shadow-glass-pop absolute left-1/2 top-12 z-40 max-w-[80%] -translate-x-1/2 truncate rounded-full bg-[var(--surface-float)] px-4 py-2 text-[12.5px] text-[var(--color-danger-strong)] backdrop-blur">{uiError ?? agentState.error}</div>
        ) : modelCatalogError ? (
          <div className="shadow-glass-pop absolute left-1/2 top-12 z-40 max-w-[80%] -translate-x-1/2 truncate rounded-full bg-[var(--surface-float)] px-4 py-2 text-[12.5px] text-[var(--color-text-secondary)] backdrop-blur">{modelCatalogError}</div>
        ) : null}

        <div className="relative min-h-0 flex-1">
          {surface === "proxy" ? <ProxyView onInspecting={setProxyInspecting} /> : surface === "thread" && activeSession ? (
            <ChatView
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
              onSignIn={beginBrowserSignIn}
              reasoningEffort={reasoningEffort}
              onReasoningEffortChange={(effort) => updateSettings(undefined, effort)}
              pendingUserMessage={pendingUserMessage}
              onVerifySecurity={() => {
                if (!activeThreadId) return;
                void window.axiomDesktop?.agent.verifySecurity(activeThreadId).catch((error: unknown) => {
                  setUiError(error instanceof Error ? error.message : String(error));
                });
              }}
            />
          ) : surface === "thread" ? (
            <div className="flex h-full items-center justify-center text-[13px] text-[var(--color-text-tertiary)]">Loading thread…</div>
          ) : (
            <WelcomeView
              draft={draft}
              onDraftChange={setDraft}
              onSubmit={() => void submitDraft()}
              models={models}
              selectedModelId={modelId || models[0]?.id || ""}
              modelStatusLabel={modelStatusLabel}
              onModelChange={(id) => { setModelId(id); preferredModelId.current = id; }}
              account={accountView}
              onSignIn={beginBrowserSignIn}
              onRefreshAccount={() => void refreshAccount()}
              reasoningEffort={reasoningEffort}
              onReasoningEffortChange={(effort) => {
                setReasoningEffort(effort);
                preferredReasoningEffort.current = effort;
              }}
            />
          )}
        </div>
      </section>
      </div>
      {settingsOpen ? (
        <SettingsSheet
          onClose={() => setSettingsOpen(false)}
          connected={agentState.connected}
          account={agentState.account}
          onRefreshAccount={() => {
            void refreshAccount();
          }}
          onLogin={beginBrowserSignIn}
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
      {signInState.kind !== "closed" ? (
        <SignInDialog
          state={signInState}
          onCancel={cancelBrowserSignIn}
          onRetry={beginBrowserSignIn}
        />
      ) : null}
    </div>
  );
}

import { MoreHorizontal, PanelLeftOpen } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import type { ClientState, ListModelsResponse } from "@axiom/axiom-acp-client";
import { ChatView } from "./components/ChatView";
import { ProxyView } from "./components/ProxyView";
import { SettingsSheet } from "./components/SettingsSheet";
import { Sidebar } from "./components/Sidebar";
import { WelcomeView } from "./components/WelcomeView";
import { WindowControls } from "./components/WindowControls";
import type { MainSurface, PendingUserMessage, ProviderModel, ReasoningEffort, Thread } from "./types";

const SIDEBAR_MIN = 220;
const SIDEBAR_MAX = 360;
const SIDEBAR_DEFAULT = 268;
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
    return {
      id: model.id,
      label,
      shortLabel,
      providerId,
      providerLabel,
      model: model.upstreamModel.trim(),
      supportedReasoningEfforts: model.thinkingLevels.filter(isReasoningEffort),
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
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT);
  const [draft, setDraft] = useState("");
  const [modelId, setModelId] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("medium");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [proxyInspecting, setProxyInspecting] = useState(false);
  const [uiError, setUiError] = useState<string | null>(null);
  const [pendingUserMessage, setPendingUserMessage] = useState<PendingUserMessage | null>(null);
  const loadingSessions = useRef(new Set<string>());
  const settingsGeneration = useRef(0);
  const preferredModelId = useRef("");
  const preferredReasoningEffort = useRef<ReasoningEffort>("medium");

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
        })),
    ];
    return summaries.map((summary) => ({
      id: summary.threadId,
      title: summary.title?.trim() || "New thread",
      folderId: collectionAssignments[summary.threadId] ?? null,
      updatedAt: relativeTime(summary.updatedAt),
      messages: [],
    }));
  }, [agentState.catalog, agentState.sessions, collectionAssignments]);

  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;
  const activeSession = activeThreadId ? agentState.sessions[activeThreadId] ?? null : null;
  const isStreaming = activeSession?.running ?? false;

  useEffect(() => {
    const api = window.axiomDesktop?.agent;
    if (!api) {
      setUiError("Axiom Desktop bridge is unavailable");
      return;
    }
    const unsubscribe = api.onState(setAgentState);
    void api.getState().then(setAgentState).catch((error: unknown) => setUiError(String(error)));
    return unsubscribe;
  }, []);

  useEffect(() => {
    if (!agentState.connected) return;
    const api = window.axiomDesktop?.agent;
    if (!api) return;
    let cancelled = false;
    const initialize = async () => {
      for (let attempt = 0; attempt < 12; attempt += 1) {
        try {
          const bootstrap = await api.desktopBootstrap();
          const [modelResponse] = await Promise.all([
            api.listModels(),
            api.listCollections(),
            api.listThreads(),
          ]);
          if (cancelled) return;
          const available = providerModels(modelResponse);
          const rememberedModel = bootstrap.newThreadSettings.model;
          const nextModel = available.some((model) => model.id === rememberedModel)
            ? rememberedModel!
            : available[0]?.id || "";
          const rememberedThinking = bootstrap.newThreadSettings.thinkingLevel;
          const nextThinking = isReasoningEffort(rememberedThinking) ? rememberedThinking : "medium";
          setModels(available);
          setModelId(nextModel);
          setReasoningEffort(nextThinking);
          preferredModelId.current = nextModel;
          preferredReasoningEffort.current = nextThinking;
          setUiError(null);
          return;
        } catch (error) {
          if (attempt === 11 && !cancelled) setUiError(error instanceof Error ? error.message : String(error));
          await new Promise((resolve) => window.setTimeout(resolve, 200));
        }
      }
    };
    void initialize();
    return () => { cancelled = true; };
  }, [agentState.connected, agentState.runtimeInstanceId]);

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
    };
    return () => { delete window.__axiomDemo; };
  }, [folders, loadThread, showProxy, showWelcome, threads]);

  const submitDraft = async () => {
    const text = draft.trim();
    const api = window.axiomDesktop?.agent;
    if (!text || !api || !agentState.connected) return;
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

  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const onMove = (moveEvent: PointerEvent) => setSidebarWidth(
      Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, startWidth + moveEvent.clientX - startX)),
    );
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  const runCollectionIntent = (intent: Promise<unknown> | undefined) => {
    void intent?.catch((error: unknown) => {
      setUiError(error instanceof Error ? error.message : String(error));
    });
  };

  return (
    <div className="flex h-full overflow-hidden bg-[var(--color-bg-base)] text-[var(--color-text-primary)]">
      {sidebarOpen ? (
        <div className="relative flex h-full shrink-0 flex-col border-r border-white/[0.045]" style={{ width: sidebarWidth }}>
          <Sidebar
            folders={folders}
            threads={threads}
            activeThreadId={surface === "thread" ? activeThreadId : null}
            proxyActive={surface === "proxy"}
            welcomeActive={surface === "welcome"}
            collapsed={!sidebarOpen}
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
            onOpenSettings={() => setSettingsOpen(true)}
          />
          <div role="separator" aria-orientation="vertical" onPointerDown={beginResize} className="absolute inset-y-0 right-0 z-20 w-1.5 cursor-col-resize hover:bg-[var(--color-cherry)]/30" />
        </div>
      ) : null}

      <section className="relative flex min-w-0 flex-1 flex-col">
        <header className="drag-region absolute inset-x-0 top-0 z-30 flex h-11 items-center bg-transparent pl-3">
          {!sidebarOpen ? <button type="button" onClick={() => setSidebarOpen(true)} className="no-drag mr-2 rounded-md p-1.5 text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-surface-hover)]" aria-label="Open sidebar"><PanelLeftOpen size={16} /></button> : null}
          <div className="min-w-0 flex-1 truncate text-[13px] font-medium tracking-tight">{title}</div>
          {surface === "thread" ? <button type="button" className="no-drag mr-1 rounded-md p-1.5 text-[var(--color-text-tertiary)]" aria-label="Thread actions"><MoreHorizontal size={16} /></button> : null}
          <WindowControls />
        </header>

        {uiError || agentState.error ? (
          <div className="absolute inset-x-4 top-12 z-40 rounded-lg border border-red-400/20 bg-red-950/90 px-3 py-2 text-[12px] text-red-200 shadow-lg">{uiError ?? agentState.error}</div>
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
              onModelChange={(id) => updateSettings(id, undefined)}
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
              onModelChange={(id) => { setModelId(id); preferredModelId.current = id; }}
              reasoningEffort={reasoningEffort}
              onReasoningEffortChange={(effort) => {
                setReasoningEffort(effort);
                preferredReasoningEffort.current = effort;
              }}
            />
          )}
          {settingsOpen ? (
            <SettingsSheet
              onClose={() => setSettingsOpen(false)}
              connected={agentState.connected}
              account={agentState.account}
              onRefreshAccount={() => {
                void window.axiomDesktop?.agent.accountStatus().catch((error: unknown) => setUiError(error instanceof Error ? error.message : String(error)));
              }}
              onLogin={() => {
                void (async () => {
                  try {
                    const started = await window.axiomDesktop?.agent.browserLoginStart();
                    if (started) await window.axiomDesktop?.agent.browserLoginComplete(started.login.loginId);
                    setUiError(null);
                  } catch (error) {
                    setUiError(error instanceof Error ? error.message : String(error));
                  }
                })();
              }}
              onLogout={() => {
                void window.axiomDesktop?.agent.logout().catch((error: unknown) => setUiError(error instanceof Error ? error.message : String(error)));
              }}
            />
          ) : null}
        </div>
      </section>
    </div>
  );
}

import { MoreHorizontal, PanelLeftOpen } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { ChatView } from "./components/ChatView";
import { ProxyView } from "./components/ProxyView";
import { SettingsSheet } from "./components/SettingsSheet";
import { Sidebar } from "./components/Sidebar";
import { WelcomeView } from "./components/WelcomeView";
import { WindowControls } from "./components/WindowControls";
import { INITIAL_FOLDERS, INITIAL_THREADS, MODELS } from "./mockData";
import type { Folder, MainSurface, ReasoningEffort, Thread } from "./types";

const SIDEBAR_MIN = 220;
const SIDEBAR_MAX = 360;
const SIDEBAR_DEFAULT = 268;

function newId(prefix: string): string {
  return `${prefix}-${Math.random().toString(36).slice(2, 9)}`;
}

export function App() {
  const [folders, setFolders] = useState<Folder[]>(INITIAL_FOLDERS);
  const [threads, setThreads] = useState<Thread[]>(INITIAL_THREADS);
  const [surface, setSurface] = useState<MainSurface>("thread");
  const [activeThreadId, setActiveThreadId] = useState<string | null>("thread-near");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT);
  const [draft, setDraft] = useState("");
  const [modelId, setModelId] = useState(MODELS[0]?.id ?? "deepseek-v4-flash");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort | undefined>(undefined);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [isStreaming, setIsStreaming] = useState(false);
  const [proxyInspecting, setProxyInspecting] = useState(false);
  const streamTimer = useRef<number | null>(null);

  const activeThread = useMemo(
    () => threads.find((thread) => thread.id === activeThreadId) ?? null,
    [threads, activeThreadId],
  );

  const title = useMemo(() => {
    if (surface === "proxy") return proxyInspecting ? "Attestation" : "Proxy";
    if (surface === "welcome" || !activeThread) return "New chat";
    return activeThread.title;
  }, [surface, activeThread, proxyInspecting]);

  const stopStream = useCallback(() => {
    if (streamTimer.current !== null) {
      window.clearTimeout(streamTimer.current);
      streamTimer.current = null;
    }
    setIsStreaming(false);
    setThreads((current) =>
      current.map((thread) => ({
        ...thread,
        messages: thread.messages.map((message) =>
          message.status === "streaming" || message.status === "pending"
            ? { ...message, status: "complete" }
            : message,
        ),
      })),
    );
  }, []);

  const showWelcome = useCallback(() => {
    stopStream();
    setSurface("welcome");
    setActiveThreadId(null);
    setDraft("");
  }, [stopStream]);

  const showProxy = useCallback(() => {
    stopStream();
    setProxyInspecting(false);
    setSurface("proxy");
  }, [stopStream]);

  const showThread = useCallback(
    (id?: string) => {
      const target = id ?? "thread-near";
      setActiveThreadId(target);
      setSurface("thread");
      setDraft("");
    },
    [],
  );

  const showFolder = useCallback(() => {
    setFolders((current) =>
      current.map((folder) =>
        folder.id === "folder-tee" ? { ...folder, collapsed: false } : folder,
      ),
    );
    showThread("thread-near");
  }, [showThread]);

  useEffect(() => {
    window.__axiomDemo = {
      showWelcome,
      showProxy,
      showFolder,
      showThread,
    };
    return () => {
      delete window.__axiomDemo;
    };
  }, [showFolder, showProxy, showThread, showWelcome]);

  const submitDraft = () => {
    const text = draft.trim();
    if (!text) return;
    setDraft("");

    const userMessage = {
      id: newId("msg"),
      role: "user" as const,
      status: "complete" as const,
      content: text,
    };
    const assistantId = newId("msg");

    if (surface !== "thread" || !activeThreadId) {
      const threadId = newId("thread");
      const thread: Thread = {
        id: threadId,
        title: text.length > 42 ? `${text.slice(0, 42)}…` : text,
        folderId: null,
        updatedAt: "just now",
        messages: [
          userMessage,
          {
            id: assistantId,
            role: "assistant",
            status: "streaming",
            content: "",
          },
        ],
      };
      setThreads((current) => [thread, ...current]);
      setActiveThreadId(threadId);
      setSurface("thread");
      setIsStreaming(true);
      streamTimer.current = window.setTimeout(() => {
        setThreads((current) =>
          current.map((item) =>
            item.id === threadId
              ? {
                  ...item,
                  messages: item.messages.map((message) =>
                    message.id === assistantId
                      ? {
                          ...message,
                          status: "complete",
                          content:
                            "This desktop build is a visual concept. The reply is canned — attestation, E2EE, and the proxy sidecar are not wired yet.",
                        }
                      : message,
                  ),
                }
              : item,
          ),
        );
        setIsStreaming(false);
      }, 900);
      return;
    }

    const threadId = activeThreadId;
    setThreads((current) =>
      current.map((thread) =>
        thread.id === threadId
          ? {
              ...thread,
              updatedAt: "just now",
              messages: [
                ...thread.messages,
                userMessage,
                {
                  id: assistantId,
                  role: "assistant",
                  status: "streaming",
                  content: "",
                },
              ],
            }
          : thread,
      ),
    );
    setIsStreaming(true);
    streamTimer.current = window.setTimeout(() => {
      setThreads((current) =>
        current.map((thread) =>
          thread.id === threadId
            ? {
                ...thread,
                messages: thread.messages.map((message) =>
                  message.id === assistantId
                    ? {
                        ...message,
                        status: "complete",
                        content:
                          "This desktop build is a visual concept. The reply is canned — attestation, E2EE, and the proxy sidecar are not wired yet.",
                      }
                    : message,
                ),
              }
            : thread,
        ),
      );
      setIsStreaming(false);
    }, 900);
  };

  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const onMove = (moveEvent: PointerEvent) => {
      const next = Math.min(
        SIDEBAR_MAX,
        Math.max(SIDEBAR_MIN, startWidth + (moveEvent.clientX - startX)),
      );
      setSidebarWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  return (
    <div className="flex h-full overflow-hidden bg-[var(--color-bg-base)] text-[var(--color-text-primary)]">
      {sidebarOpen ? (
        <div
          className="relative flex h-full shrink-0 flex-col border-r border-white/[0.045]"
          style={{ width: sidebarWidth }}
        >
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
            onSelectThread={(id) => {
              setActiveThreadId(id);
              setSurface("thread");
            }}
            onCreateFolder={() => {
              const id = newId("folder");
              setFolders((current) => [
                ...current,
                { id, name: "Untitled folder", collapsed: false },
              ]);
            }}
            onToggleFolder={(id) => {
              setFolders((current) =>
                current.map((folder) =>
                  folder.id === id ? { ...folder, collapsed: !folder.collapsed } : folder,
                ),
              );
            }}
            onRenameFolder={(id, name) => {
              setFolders((current) =>
                current.map((folder) => (folder.id === id ? { ...folder, name } : folder)),
              );
            }}
            onDeleteFolder={(id) => {
              setFolders((current) => current.filter((folder) => folder.id !== id));
              setThreads((current) =>
                current.map((thread) =>
                  thread.folderId === id ? { ...thread, folderId: null } : thread,
                ),
              );
            }}
            onDeleteThread={(id) => {
              setThreads((current) => current.filter((thread) => thread.id !== id));
              if (activeThreadId === id) showWelcome();
            }}
            onMoveThread={(threadId, folderId) => {
              setThreads((current) =>
                current.map((thread) =>
                  thread.id === threadId ? { ...thread, folderId } : thread,
                ),
              );
              if (folderId) {
                setFolders((current) =>
                  current.map((folder) =>
                    folder.id === folderId ? { ...folder, collapsed: false } : folder,
                  ),
                );
              }
            }}
            onOpenSettings={() => setSettingsOpen(true)}
          />
          <div
            role="separator"
            aria-orientation="vertical"
            onPointerDown={beginResize}
            className="absolute inset-y-0 right-0 z-20 w-1.5 cursor-col-resize hover:bg-[var(--color-cherry)]/30"
          />
        </div>
      ) : null}

      <section className="relative flex min-w-0 flex-1 flex-col">
        <header className="drag-region absolute inset-x-0 top-0 z-30 flex h-11 items-center bg-transparent pl-3">
          {!sidebarOpen ? (
            <button
              type="button"
              onClick={() => setSidebarOpen(true)}
              className="no-drag mr-2 rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
              aria-label="Open sidebar"
            >
              <PanelLeftOpen size={16} />
            </button>
          ) : null}
          <div className="min-w-0 flex-1 truncate text-[13px] font-medium tracking-tight text-[var(--color-text-primary)] drop-shadow-[0_1px_8px_rgba(14,14,16,0.85)]">
            {title}
          </div>
          {surface === "thread" ? (
            <button
              type="button"
              className="no-drag mr-1 shrink-0 rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
              aria-label="Thread actions"
            >
              <MoreHorizontal size={16} />
            </button>
          ) : null}
          <WindowControls />
        </header>

        <div className="relative min-h-0 flex-1">
          {surface === "proxy" ? (
            <ProxyView onInspecting={setProxyInspecting} />
          ) : surface === "thread" && activeThread ? (
            <ChatView
              thread={activeThread}
              draft={draft}
              onDraftChange={setDraft}
              onSubmit={submitDraft}
              onStop={stopStream}
              isStreaming={isStreaming}
              models={MODELS}
              selectedModelId={modelId}
              onModelChange={setModelId}
              reasoningEffort={reasoningEffort}
              onReasoningEffortChange={setReasoningEffort}
            />
          ) : (
            <WelcomeView
              draft={draft}
              onDraftChange={setDraft}
              onSubmit={submitDraft}
              models={MODELS}
              selectedModelId={modelId}
              onModelChange={setModelId}
              reasoningEffort={reasoningEffort}
              onReasoningEffortChange={setReasoningEffort}
            />
          )}
          {settingsOpen ? <SettingsSheet onClose={() => setSettingsOpen(false)} /> : null}
        </div>
      </section>
    </div>
  );
}

import { Folder, PanelLeftOpen } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { BalanceScreen } from "./components/BalanceScreen";
import { ChatView } from "./components/ChatView";
import { DeleteConfirmationDialog } from "./components/DeleteConfirmationDialog";
import { DismissibleNotice } from "./components/DismissibleNotice";
import { MessageQueue } from "./components/MessageQueue";
import { OverflowTitle } from "./components/OverflowTitle";
import { ProxyView } from "./components/ProxyView";
import { RenameDialog } from "./components/RenameDialog";
import type { SettingsCategory } from "./components/SettingsPanel";
import { SettingsSheet } from "./components/SettingsSheet";
import { Sidebar } from "./components/Sidebar";
import { SignInStatus } from "./components/SignInStatus";
import { TrafficLightGhost } from "./components/TrafficLightGhost";
import { UpdateNotice } from "./components/UpdateNotice";
import { WebConsentDialog } from "./components/WebConsentDialog";
import { WelcomeView } from "./components/WelcomeView";
import { WindowControls } from "./components/WindowControls";
import { setThemePreference, useTheme } from "./lib/theme";
import { isMac, MAC_TRAFFIC_LIGHT_INSET_CLASS, useFullScreen } from "./lib/window-chrome";
import { useDesktopUpdates } from "./useDesktopUpdates";
import { useChatFileDrop } from "./useChatFileDrop";

import { useConversationNavigation } from "./useConversationNavigation";
import { useConversations } from "./useConversations";
import { useDesktopState } from "./useDesktopState";
import { useModelCatalog } from "./useModelCatalog";
import { useNativeAccount } from "./useNativeAccount";

const SIDEBAR_WIDTH = 260;

export function App() {
  const updates = useDesktopUpdates();
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>("appearance");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const fullScreen = useFullScreen();

  useTheme();

  const [accountScreen, setAccountScreen] = useState<"settings" | "balance" | null>(null);
  const accountScreenOpen = accountScreen !== null;
  const accountScreenReturnFocus = useRef<HTMLElement | null>(null);
  const [proxyInspecting, setProxyInspecting] = useState(false);
  const [uiError, setUiError] = useState<string | null>(null);
  const { agentState, errorRevision } = useDesktopState(setUiError);
  const navigation = useConversationNavigation();
  const { surface, setSurface, activeThreadId, cancelThreadPreparation } = navigation;
  const catalog = useModelCatalog(agentState, navigation, setUiError);
  const { models, modelId, effectiveReasoningEffort, modelCatalogError, modelStatusLabel, updateSettings } = catalog;
  const closeAccountScreen = useCallback(() => setAccountScreen(null), []);
  const account = useNativeAccount(agentState, catalog.desktopReady, catalog.refreshModels, catalog.clearModels, closeAccountScreen, setUiError);
  const { accountView, requiresSignIn, signInState, setSignInState, refreshAccount, logout, refreshBilling, cancelNativeSignIn, openSignIn, openAccountPortal } = account;
  const conversations = useConversations(agentState, navigation, catalog, requiresSignIn, openSignIn, setUiError);
  const { reviewPreparation, attachmentDraft, draft, setDraft, threadPreparation, pendingUserMessage, threadLoadError,
    deleteTarget, setDeleteTarget, renameTarget, setRenameTarget, billingAccountId, webAccess,
    threads, folders, activeThread, activeSession, agentAccountScope, agentControl, isStreaming,
    ensureThreadLoaded, loadThread, showWelcome, reviseMessage, submitDraft, stopStream, deleteThread,
    messageQueue, threadLoads } = conversations;
  const composeDraft = (text: string) => {
    setDraft(text);
    const selectedModel = modelId || models[0]?.id;
    if (text.trim() && selectedModel && agentState.connected && !requiresSignIn && !isStreaming) {
      // Sends only the model ID; draft text stays on this device.
      void window.axiomDesktop?.agent.prewarmSecurity(selectedModel).catch(() => undefined);
    }
  };
  const preparation = threadPreparation?.accountId === billingAccountId
    && threadPreparation?.runtimeId === agentState.runtimeInstanceId ? threadPreparation : null;
  const errorNotice = uiError ? { source: "ui", text: uiError }
    : webAccess.error ? { source: "web", text: webAccess.error }
    : agentState.error ? { source: "agent", text: agentState.error }
    : modelCatalogError ? { source: "catalog", text: modelCatalogError } : null;
  const fileDrop = useChatFileDrop(
    !accountScreenOpen && !preparation && (surface === "welcome" || (surface === "thread" && !!activeSession)) && !attachmentDraft.attachmentsBusy,
    attachmentDraft.attachmentScope + surface,
    (files) => void attachmentDraft.addFiles(files),
  );

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

  const title = surface === "proxy"
    ? (proxyInspecting ? "Attestation" : "Proxy")
    : surface === "welcome" || !activeThread
      ? "New thread"
      : activeThread.title;

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
            if (thread && !threadLoads.isBlocked(id)) setRenameTarget({ kind: "thread", id, title: thread.title });
          }}
          onDeleteFolder={(id) => {
            const folder = folders.find((candidate) => candidate.id === id);
            if (folder) setDeleteTarget({ kind: "folder", id, title: folder.name });
          }}
          onDeleteThread={(id) => {
            const thread = threads.find((candidate) => candidate.id === id);
            if (thread && !threadLoads.isBlocked(id)) {
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
          onLogout={logout}
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
        <section data-chat-drop-zone {...fileDrop.handlers} className="stage-card absolute inset-px flex flex-col overflow-hidden rounded-[21px] bg-[var(--surface-stage)]">
          {fileDrop.dragging ? <div data-file-drop-overlay className="pointer-events-none absolute inset-3 z-50 flex items-center justify-center rounded-[16px] border-2 border-dashed border-[var(--color-cherry)] bg-[var(--surface-stage)]/90">
            <div className="rounded-xl bg-[var(--surface-float)] px-6 py-4 text-center text-[15px] font-medium text-[var(--color-text-primary)]">Drop files to attach</div>
          </div> : null}
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

          {errorNotice ? (
            <DismissibleNotice key={JSON.stringify([billingAccountId, agentState.runtimeInstanceId, errorNotice.source, errorNotice.text, errorNotice.source === "agent" ? errorRevision : 0])}
              onDismiss={errorNotice.source === "ui" ? () => setUiError(null) : undefined}
              className={`shadow-glass-pop absolute left-1/2 top-12 z-40 max-w-[80%] -translate-x-1/2 rounded-xl bg-[var(--surface-float)] px-4 py-2 text-[12.5px] ${errorNotice.source === "catalog" ? "text-[var(--color-text-secondary)]" : "text-[var(--color-danger-strong)]"} backdrop-blur`}>
              <span className="min-w-0 break-words">{errorNotice.text}</span>
            </DismissibleNotice>
          ) : null}

          <div className="relative min-h-0 flex-1">
            {surface === "proxy" ? <ProxyView accountId={agentState.connected ? billingAccountId : null} runtimeId={agentState.runtimeInstanceId} onInspecting={setProxyInspecting} /> : surface === "thread" && (preparation || activeSession) ? (
              <ChatView
                preparation={preparation}
                attachmentPreviews={conversations.attachmentPreviews}
                onRevise={!isStreaming && agentState.account?.state === "valid" ? reviseMessage : undefined}
                agentControl={agentControl}
                queue={<MessageQueue messages={messageQueue.list(activeThreadId, billingAccountId).filter((message) => message.id !== pendingUserMessage?.id || message.status !== "sending")}
                  paused={!!activeThreadId && messageQueue.isPaused(activeThreadId)}
                  canSend={agentState.connected && !!billingAccountId && !messageQueue.isStopping(activeThreadId!)}
                  onSendNow={(id) => messageQueue?.sendNow(id)} onRemove={(id) => messageQueue?.remove(id)} />}
                webEnabled={webAccess.enabled}
                webDisabled={!billingAccountId || !agentState.connected}
                onWebToggle={webAccess.toggle}
                session={activeSession}
                attachments={{ items: attachmentDraft.attachments, busy: attachmentDraft.attachmentsBusy, onAddFiles: (files) => void attachmentDraft.addFiles(files), onRemove: (index) => attachmentDraft.setAttachments(attachmentDraft.attachments.filter((_, current) => current !== index)) }}
                draft={draft}
                onDraftChange={composeDraft}
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
                onAcceptOutdatedTee={() => conversations.acceptOutdatedTee(modelId)}
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
                savedMessages={<MessageQueue messages={messageQueue.preparations(billingAccountId)} paused canSend={agentState.connected && !draft.trim()}
                  onSendNow={(id) => void reviewPreparation(id)} onRemove={(id) => messageQueue?.remove(id)} />}
                agentControl={agentControl}
                webEnabled={webAccess.enabled}
                webDisabled={!billingAccountId || !agentState.connected}
                onWebToggle={webAccess.toggle}
                attachments={{ items: attachmentDraft.attachments, busy: attachmentDraft.attachmentsBusy, onAddFiles: (files) => void attachmentDraft.addFiles(files), onRemove: (index) => attachmentDraft.setAttachments(attachmentDraft.attachments.filter((_, current) => current !== index)) }}
                draft={draft}
                onDraftChange={composeDraft}
                onSubmit={() => void submitDraft()}
                models={models}
                selectedModelId={modelId || models[0]?.id || ""}
                modelStatusLabel={modelStatusLabel}
                onModelChange={catalog.selectNewModel}
                account={accountView}
                signInActive={signInState.kind !== "closed"}
                onSignIn={openSignIn}
                onRefreshAccount={() => void refreshAccount()}
                reasoningEffort={effectiveReasoningEffort}
                onReasoningEffortChange={catalog.selectNewThinking}
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
          onManageAccount={openAccountPortal}
          onLogout={logout}
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
          onRefreshBilling={refreshBilling}
          onLogin={openSignIn}
        />
      ) : null}
    </div>
  );
}

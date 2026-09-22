import { acceptsAttachment } from "./attachments";
import { AttachmentPreviews } from "./attachmentPreviews";
import type { ClientState } from "@axiom/axiom-acp-client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DEFAULT_AGENT_SELECTION, type AgentControlOptions, type AgentSelection } from "./components/AgentModeControl";
import { MessageQueue as MessageQueueController } from "./messageQueue";
import { desktopErrorMessage } from "./signInFlow";
import { ThreadLoadTracker } from "./threadLoading";
import { compareThreadMessages, newestMessageAt, orderFoldersByMessages } from "./threadOrdering";
import type { PendingUserMessage, Thread } from "./types";
import { useAttachmentDraft } from "./useAttachmentDraft";
import { useAccountDraft } from "./useAccountDraft";
import { useWebAccess } from "./useWebAccess";
import { browserConsentStorage } from "./webConsent";

import { type useConversationNavigation } from "./useConversationNavigation";
import { type useModelCatalog } from "./useModelCatalog";

function relativeTime(value: string): string {
  const elapsed = Date.now() - Date.parse(value);
  if (!Number.isFinite(elapsed) || elapsed < 60_000) return "just now";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}m`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}h`;
  return `${Math.floor(elapsed / 86_400_000)}d`;
}

export function useConversations(agentState: ClientState, navigation: ReturnType<typeof useConversationNavigation>,
  catalog: ReturnType<typeof useModelCatalog>, requiresSignIn: boolean,
  openSignIn: () => void, setUiError: (error: string | null) => void) {
  const { surface, setSurface, activeThreadId, setActiveThreadId, activeThreadIdRef,
    navigationRevision, threadPreparation, setThreadPreparation, cancelThreadPreparation } = navigation;
  const { desktopReady, selectedModel, effectiveReasoningEffort, rememberSelection, restorePreferredSelection } = catalog;
  const [draft, setDraft, writeDraft, writeDraftDurable] = useAccountDraft(agentState.account?.state === "valid" ? agentState.account.account?.id ?? null : null, surface === "thread" ? activeThreadId : null);
  const attachmentDraft = useAttachmentDraft(agentState.account?.state === "valid" ? agentState.account.account?.id ?? null : null, surface === "thread" ? activeThreadId : null, setUiError);
  const savingDraft = useRef(false);
  const [queueRevision, setQueueRevision] = useState(0);
  const [newAgentDraft, setNewAgentDraft] = useState<{ scope: string; selection: AgentSelection; } | null>(null);
  const [agentSettingsBusy, setAgentSettingsBusy] = useState(false);
  const messageQueue = useRef<MessageQueueController | null>(null);
  const submittingDraft = useRef(new Map<string, string>());

  if (!messageQueue.current) messageQueue.current = new MessageQueueController(browserConsentStorage(), () => setQueueRevision((value) => value + 1), setUiError);

  const threadLoads = useRef(new ThreadLoadTracker());
  const [threadLoadError, setThreadLoadError] = useState<{ id: string; detail: string; } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ kind: "thread" | "folder"; id: string; title: string; } | null>(null);
  const [renameTarget, setRenameTarget] = useState<{ kind: "thread" | "folder"; id: string; title: string; } | null>(null);
  const seenMessageTimes = useRef(new Map<string, number>());
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
          lastUserMessageAt: session.lastUserMessageAt ?? null,
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
        lastUserMessageAt: newestMessageAt(summary.lastUserMessageAt, session?.lastUserMessageAt),
        status: (working ? "working" : unread ? "unread" : "idle") as Thread["status"],
        messages: [],
      };
    }).sort(compareThreadMessages);
  }, [agentState.catalog, agentState.sessions, collectionAssignments, activeThreadId, surface]);

  const folders = useMemo(() => orderFoldersByMessages(unsortedFolders, threads), [unsortedFolders, threads]);
  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;
  const activeSession = activeThreadId ? agentState.sessions[activeThreadId] ?? null : null;
  // An outbox entry that is already dispatched belongs in the transcript. This
  // also covers existing chats, retries and queued work as it starts sending.
  const pendingUserMessage = useMemo<PendingUserMessage | null>(() => {
    if (!agentState.connected || !activeSession) return null;
    const sending = messageQueue.current!.list(activeThreadId, billingAccountId).find((item) => item.status === "sending"
      && !activeSession.timeline.some((row) => row.clientItemId === item.id));
    return sending ? { id: sending.id, sessionId: sending.threadId, text: sending.text, attachments: sending.attachments } : null;
  }, [agentState.connected, activeSession, activeThreadId, billingAccountId, queueRevision]);
  const agentAccountScope = JSON.stringify([billingAccountId, agentState.runtimeInstanceId]);
  const attachmentPreviews = useMemo(() => new AttachmentPreviews(), [agentAccountScope]);
  // Restored queued messages also hand their local bytes to the transcript.
  for (const message of messageQueue.current.list(activeThreadId, billingAccountId)) {
    attachmentPreviews.remember(message.id, message.attachments ?? []);
  }
  const newAgentSelection = newAgentDraft?.scope === agentAccountScope ? newAgentDraft.selection : DEFAULT_AGENT_SELECTION;
  const currentAgent = surface === "thread" ? activeSession?.desktopAgent : null;
  const agentControl: AgentControlOptions = {
    scope: JSON.stringify([agentAccountScope, surface === "thread" ? activeThreadId : "new"]),
    selection: currentAgent ? {
      enabled: currentAgent.enabled, permission: currentAgent.permission,
      workingDirectory: currentAgent.usesDefaultDirectory ? null : currentAgent.workingDirectory
    } : surface === "thread" ? DEFAULT_AGENT_SELECTION : newAgentSelection,
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

  const isStreaming = !!activeSession?.running || !!pendingUserMessage;

  useEffect(() => {
    threadLoads.current.reset();
    setThreadLoadError(null);
    setDeleteTarget(null);
    setRenameTarget(null);
  }, [agentState.runtimeInstanceId, billingAccountId, agentState.connected]);

  useEffect(() => {
    if (!threadPreparation || (agentState.connected && agentState.account?.state === "valid"
      && threadPreparation.accountId === billingAccountId && threadPreparation.runtimeId === agentState.runtimeInstanceId)) return;
    cancelThreadPreparation();
    setSurface("welcome");
    setUiError("The account or connection changed. Nothing was sent.");
  }, [threadPreparation, billingAccountId, agentState.connected, agentState.account?.state, agentState.runtimeInstanceId, cancelThreadPreparation, setDraft]);

  useEffect(() => () => { submittingDraft.current.clear(); }, []);

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

  const showWelcome = useCallback(() => {
    cancelThreadPreparation();
    resetNewWeb();
    setNewAgentDraft(null);
    activeThreadIdRef.current = null;
    setSurface("welcome");
    setActiveThreadId(null);
    setThreadLoadError(null);
    setUiError(null);
    restorePreferredSelection();
  }, [resetNewWeb, cancelThreadPreparation, setDraft, restorePreferredSelection]);

  useEffect(() => {
    if (agentState.connected && desktopReady && surface === "thread" && activeThreadId
      && !activeThread && pendingUserMessage?.sessionId !== activeThreadId) {
      showWelcome();
    }
  }, [agentState.connected, desktopReady, surface, activeThreadId, activeThread, pendingUserMessage, showWelcome]);

  const reviseMessage = async (userItemId: string, text: string) => {
    const api = window.axiomDesktop?.agent;
    if (!api?.revisePrompt || !activeThreadId || !activeSession || activeSession.running) {
      const error = new Error("Restart Axiom and wait for the current reply before editing.");
      setUiError(error.message); throw error;
    }
    if (messageQueue.current!.list(activeThreadId, billingAccountId).length) {
      const error = new Error("Send or remove queued messages before editing this conversation.");
      setUiError(error.message); throw error;
    }
    const owner = billingAccountId;
    const threadId = activeThreadId;
    setUiError(null);
    try {
      await api.revisePrompt(threadId, text, userItemId, activeSession.threadRevision, webAccess.enabled, activeSession.desktopAgent?.revision ?? 0);
    } catch (error) {
      const current = await api.getState();
      if (current.account?.account?.id === owner && activeThreadIdRef.current === threadId) setUiError(error instanceof Error ? error.message : String(error));
      throw error;
    }
  };

  const acceptOutdatedTee = async (expectedModelId: string) => {
    const api = window.axiomDesktop?.agent;
    const threadId = activeThreadId;
    const modelId = activeSession?.settings?.model;
    const owner = billingAccountId;
    const runtime = agentState.runtimeInstanceId;
    if (!api || !threadId || !modelId || modelId !== expectedModelId || activeSession?.security?.state !== "outdated" || activeSession.running) return;
    await api.verifySecurity(threadId, true, modelId);
    const current = await api.getState();
    const session = current.sessions[threadId];
    if (current.runtimeInstanceId !== runtime || current.account?.account?.id !== owner
      || activeThreadIdRef.current !== threadId || session?.settings?.model !== modelId || session.running) return;
    if (session.security?.state !== "verified" && session.security?.state !== "degraded") return;
    setUiError(null);
    const queuedMessages = messageQueue.current!.list(threadId, owner);
    const queued = queuedMessages.find((item) => item.status === "queued" && item.error?.includes("OutOfDate"));
    if (queued) { messageQueue.current!.sendNow(queued.id); return; }
    if (queuedMessages.length) return;
    // Retry through local edit/regenerate, preserving the original attachments.
    const last = session.timeline.at(-1);
    const user = [...session.timeline].reverse().find((item) => item.kind === "user");
    if (last?.kind === "error" && last.text.includes("OutOfDate") && user && user.turnId === last.turnId) {
      await api.revisePrompt(threadId, user.text, user.id, session.threadRevision, webAccess.enabled, session.desktopAgent?.revision ?? 0);
    }
  };

  const submitDraft = async () => {
    const text = draft.trim();
    const api = window.axiomDesktop?.agent;
    const viewRevision = navigationRevision.current;
    const scope = `${billingAccountId}:${agentState.runtimeInstanceId}:${viewRevision}:${activeThreadId ?? "new"}`;
    if ((!text && !attachmentDraft.attachments.length) || attachmentDraft.attachmentsBusy || savingDraft.current || !api || !agentState.connected || [...submittingDraft.current.values()].includes(scope) || agentSettingsBusy) return;
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
    const files = attachmentDraft.attachments;
    if (files.some((file) => !acceptsAttachment(selectedModel, file))) {
      setUiError("Choose a model that supports these files before sending."); return;
    }
    if (files.length && !api.promptWithAttachments) { setUiError("Restart Axiom to enable attachments. Your draft was kept."); return; }
    const webEnabled = webAccess.enabled;
    const creatingThread = surface !== "thread" || !activeThreadId;
    let submissionId: string;
    try {
      messageQueue.current!.sync(agentState, api);
      savingDraft.current = true;
      submissionId = await messageQueue.current!.reserveDurable(creatingThread ? null : activeThreadId, text, webEnabled, files);
    } catch (error) {
      setUiError(desktopErrorMessage(error, "Your message could not be saved. Your draft was kept."));
      return;
    } finally { savingDraft.current = false; }
    submittingDraft.current.set(submissionId, scope);
    attachmentPreviews.remember(submissionId, files);
    // Clear only the captured draft. A navigation/account change or newer typing
    // during the disk commit must never clear another composer's text.
    writeDraft(creatingThread ? null : activeThreadId, (previous) => previous.trim() === text ? "" : previous);
    attachmentDraft.clearAttachments(files);
    const stillSelected = () => navigationRevision.current === viewRevision;
    rememberSelection(selectedModel.id, effectiveReasoningEffort);
    setUiError(null);
    if (creatingThread && stillSelected()) {
      // Navigation can dismiss this view without cancelling the durable submission.
      setThreadPreparation({ id: submissionId, text, attachments: files, accountId: billingAccountId, runtimeId: agentState.runtimeInstanceId });
      activeThreadIdRef.current = null;
      setActiveThreadId(null);
      setThreadLoadError(null);
      setSurface("thread");
    }
    const currentSubmissionState = async () => {
      if (!submittingDraft.current.has(submissionId)) return null;
      const current = await api.getState();
      if (!submittingDraft.current.has(submissionId)) return null;
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
        messageQueue.current!.bindPreparation(submissionId, sessionId);
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
      if (creatingThread) webAccess.adoptThread(sessionId, webEnabled, stillSelected());
      messageQueue.current!.sync(current, api);
      messageQueue.current!.ready(submissionId, sessionId);
      if (creatingThread && stillSelected()) {
        activeThreadIdRef.current = sessionId;
        setActiveThreadId(sessionId);
        setNewAgentDraft(null);
        setThreadPreparation(null);
      }
      if (stillSelected()) setUiError(null);
    } catch (error) {
      // A late completion must never reopen a thread, restore an old draft, or
      // overwrite the error belonging to a newer submission/account.
      if (!submittingDraft.current.has(submissionId)) return;
      messageQueue.current!.failPreparation(submissionId, desktopErrorMessage(error, "Thread setup failed. Your message is saved for review."));
      if (!stillSelected()) return;
      const current = await api.getState().catch(() => null);
      if (current?.account?.account?.id !== billingAccountId || current?.runtimeInstanceId !== agentState.runtimeInstanceId) return;
      try {
        if (!files.length && writeDraft(creatingThread ? null : activeThreadId, (previous) => previous ? `${previous}\n\n${text}` : text, true)) {
          messageQueue.current!.remove(submissionId);
        }
      } catch { /* Keep the durable outbox entry available for review. */ }
      if (creatingThread) {
        setThreadPreparation(null);
        setSurface("welcome");
      }
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      submittingDraft.current.delete(submissionId);
    }
  };

  const reviewPreparation = async (id: string) => {
    const item = messageQueue.current!.preparations(billingAccountId).find((entry) => entry.id === id);
    if (!item || item.status !== "queued" || draft.trim() || attachmentDraft.attachments.length || attachmentDraft.attachmentsBusy) return;
    try {
      if (!await writeDraftDurable(null, item.text)) return;
      if (!await attachmentDraft.setAttachmentsDurable(item.attachments ?? [])) return;
      messageQueue.current!.remove(id);
    } catch { setUiError("The draft could not be saved. Your message is still in the queue."); }
  };

  const stopStream = useCallback(() => {
    if (activeThreadId) {
      messageQueue.current?.pause(activeThreadId);
      void window.axiomDesktop?.agent.cancel(activeThreadId);
    }
  }, [activeThreadId]);

  const deleteThread = async (id: string) => {
    const api = window.axiomDesktop?.agent;
    if (!api || threadLoads.current.isBlocked(id)) return;
    // Invalidate selection and pending loads before the first IPC await: the
    // sidecar publishes removal before deleteConfirm finishes its refresh.
    threadLoads.current.block(id);
    if (activeThreadIdRef.current === id) showWelcome();
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

  return { acceptOutdatedTee, reviewPreparation, attachmentDraft, attachmentPreviews, draft, setDraft, writeDraft, threadPreparation, pendingUserMessage, threadLoadError, deleteTarget, setDeleteTarget, renameTarget, setRenameTarget, billingAccountId, webAccess, threads, folders, activeThread, activeSession, agentAccountScope, agentControl, isStreaming, ensureThreadLoaded, loadThread, showWelcome, reviseMessage, submitDraft, stopStream, deleteThread, messageQueue: messageQueue.current, threadLoads: threadLoads.current };
}

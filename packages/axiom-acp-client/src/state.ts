import { EventEmitter } from "node:events";
import type {
  AccountStatus,
  ActivityEvent,
  BillingStatus,
  CollectionState,
  ContextUsage,
  RequestUsage,
  DeliveryMetadata,
  EventNotification,
  GetThreadTimelineResponse,
  DesktopAgentSettings,
  ConfigureDesktopAgentResponse,
  ProfilePreferences,
  SecurityStatus,
  SecurityEvidence,
  ThreadSummary,
  TimelineItem as DurableTimelineItem,
} from "./generated/protocol.js";
import type { PendingInteraction, SessionConfigOption, SessionMode } from "./types.js";

export type ToolActivity = {
  callId: string;
  name?: string;
  title: string;
  kind: string;
  status: string;
  input: unknown;
  content: unknown[];
  locations: unknown[];
};

export type ClientTimelineItem = {
  id: string;
  turnId?: string;
  clientItemId?: string;
  sequence?: number;
  kind: "user" | "assistant" | "reasoning" | "tool" | "plan" | "activity" | "error";
  text: string;
  status?: string;
  /// True only when the authoritative durable item carries evidence that its
  /// exact terminal provider response was verified. Session preflight state
  /// is deliberately not promoted to this per-response claim.
  terminalVerified?: boolean;
  finishReason?: string;
  raw?: unknown;
  tool?: ToolActivity;
};

export interface ThreadSettings {
  model: string;
  thinkingLevel: string;
  permissionProfile: string;
}

export interface ClientSessionState {
  sessionId: string;
  title: string | null;
  cwd: string;
  settings: ThreadSettings | null;
  security: SecurityStatus | null;
  /** Public report from the last explicit/local preflight; never response-receipt proof. */
  securityEvidence?: SecurityEvidence | null;
  securityVerificationPending?: boolean;
  securityVerificationError?: string | null;
  contextUsage?: ContextUsage | null;
  requestUsage?: RequestUsage[];
  modes: SessionMode[];
  currentModeId: string | null;
  configOptions: SessionConfigOption[];
  timeline: ClientTimelineItem[];
  interactions: PendingInteraction[];
  running: boolean;
  activeTurnId?: string | null;
  desktopAgent?: DesktopAgentSettings | null;
  needsResync: boolean;
  threadRevision: number;
  lastTimelineSequence: number;
  lastMessageAt?: string | null;
  lastUserMessageAt?: string | null;
}

export interface ClientState {
  connected: boolean;
  runtimeInstanceId: string | null;
  lastSequence: number;
  sessions: Record<string, ClientSessionState>;
  catalog: ThreadSummary[];
  collections: CollectionState;
  preferences: ProfilePreferences | null;
  account: AccountStatus | null;
  billing: BillingStatus | null;
  diagnostic: string;
  error: string | null;
}

export interface ExtensionEventResult {
  threadId: string | null;
  fullResync: boolean;
}

function emptySession(sessionId: string, cwd = ""): ClientSessionState {
  return {
    sessionId,
    title: null,
    cwd,
    settings: null,
    security: null,
    contextUsage: null,
    requestUsage: [],
    modes: [],
    currentModeId: null,
    configOptions: [],
    timeline: [],
    interactions: [],
    running: false,
    needsResync: false,
    threadRevision: 0,
    lastTimelineSequence: 0,
    lastMessageAt: null,
    lastUserMessageAt: null,
  };
}

function textFromContent(value: unknown): string {
  if (!value || typeof value !== "object") return "";
  const record = value as Record<string, unknown>;
  if (typeof record.text === "string") return record.text;
  if (record.content && typeof record.content === "object") {
    const content = record.content as Record<string, unknown>;
    if (typeof content.text === "string") return content.text;
  }
  return "";
}

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function deliveryMetadata(record: Record<string, unknown>, update: Record<string, unknown>): DeliveryMetadata | null {
  const topMeta = object(record._meta);
  const updateMeta = object(update._meta);
  const contentMeta = object(object(update.content)?._meta);
  const candidate = object(topMeta?.axiom) ?? object(updateMeta?.axiom) ?? object(contentMeta?.axiom);
  if (!candidate) return null;
  if (typeof candidate.threadRevision !== "number" || typeof candidate.lastTimelineSequence !== "number") {
    return null;
  }
  return candidate as DeliveryMetadata;
}

function mergeTool(
  previous: ToolActivity | undefined,
  callId: string,
  update: Record<string, unknown>,
): ToolActivity {
  const content = Array.isArray(update.content) ? update.content : [];
  const locations = Array.isArray(update.locations) ? update.locations : [];
  return {
    callId,
    name: typeof update.title === "string" && /(?:^| · )[a-z][a-z0-9_]*$/.test(update.title)
      ? update.title.split(" · ").at(-1) : previous?.name,
    title: typeof update.title === "string" ? update.title : (previous?.title ?? "Tool activity"),
    kind: typeof update.kind === "string" ? update.kind : (previous?.kind ?? "other"),
    status: typeof update.status === "string" ? update.status : (previous?.status ?? "pending"),
    input: update.rawInput ?? previous?.input ?? null,
    content: content.length > 0 ? content : (previous?.content ?? []),
    locations: locations.length > 0 ? locations : (previous?.locations ?? []),
  };
}

function durableTool(item: DurableTimelineItem): ToolActivity {
  const metadata = object(item.metadata) ?? {};
  const content = Array.isArray(metadata.content) ? [...metadata.content] : [];
  if (
    item.content
    && !content.some((entry) => textFromContent(entry) === item.content)
  ) {
    content.push({ type: "text", text: item.content });
  }
  const locations = Array.isArray(metadata.locations)
    ? metadata.locations
    : Array.isArray(metadata.files)
      ? metadata.files
      : [];
  return {
    callId: item.externalId ?? item.id,
    name: typeof metadata.name === "string" ? metadata.name : undefined,
    title: typeof metadata.title === "string"
      ? metadata.title
      : typeof metadata.name === "string"
        ? metadata.name
        : "Tool activity",
    kind: typeof metadata.kind === "string" ? metadata.kind : "other",
    status: item.status,
    input: metadata.arguments ?? metadata.input ?? null,
    content,
    locations,
  };
}

function fromDurable(item: DurableTimelineItem): ClientTimelineItem {
  const clientItemId = item.clientItemId ?? undefined;
  const metadata = object(item.metadata) ?? {};
  const terminalVerified = metadata.terminalVerified === true
    || metadata.terminal_verified === true;
  switch (item.kind) {
    case "user_message":
      return { id: item.id, turnId: item.turnId ?? undefined, clientItemId, sequence: item.sequence, kind: "user", text: item.content, status: item.status, raw: item };
    case "assistant_message":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "assistant", text: item.content, status: item.status, terminalVerified, finishReason: typeof metadata.finish_reason === "string" ? metadata.finish_reason : undefined, raw: item };
    case "reasoning":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "reasoning", text: item.content, status: item.status, terminalVerified, finishReason: typeof metadata.finish_reason === "string" ? metadata.finish_reason : undefined, raw: item };
    case "tool_call": {
      const tool = durableTool(item);
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "tool", text: tool.title, status: item.status, raw: item, tool };
    }
    case "plan":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "plan", text: item.content || "Plan updated", status: item.status, raw: item };
    case "notice":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: item.status === "failed" ? "error" : "activity", text: item.content, status: item.status, raw: item };
    default:
      // Timeline pages are durable state. Preserve an additive item kind as a
      // generic activity instead of dropping it (or inserting `undefined`).
      return {
        id: item.id,
        turnId: item.turnId ?? undefined,
        clientItemId,
        sequence: item.sequence,
        kind: item.status === "failed" ? "error" : "activity",
        text: item.content || `Unsupported timeline item: ${String(item.kind)}`,
        status: item.status,
        raw: item,
      };
  }
}

function messageTurnId(itemId: string, kind: ClientTimelineItem["kind"]): string | undefined {
  const prefix = kind === "assistant"
    ? "assistant:"
    : kind === "reasoning"
      ? "reasoning:"
      : kind === "user"
        ? "user:"
        : "";
  if (!prefix || !itemId.startsWith(prefix) || itemId.length === prefix.length) return undefined;
  return itemId.slice(prefix.length);
}

function settingsFromThread(thread: ThreadSummary): ThreadSettings {
  return {
    model: thread.selectedModel ?? "",
    thinkingLevel: thread.thinkingLevel,
    permissionProfile: thread.profile,
  };
}

export class AxiomStateStore extends EventEmitter {
  private accountContextGeneration = 0;
  private securityContexts = new WeakMap<ClientSessionState, object>();
  // Text already included by either a stream update or an authoritative page.
  private textRevisions = new Map<string, number>();
  private value: ClientState = {
    connected: false,
    runtimeInstanceId: null,
    lastSequence: 0,
    sessions: {},
    catalog: [],
    collections: { revision: 0, collections: [] },
    preferences: null,
    account: null,
    billing: null,
    diagnostic: "",
    error: null,
  };

  snapshot(): ClientState {
    return structuredClone(this.value);
  }

  accountContextToken(): number {
    return this.accountContextGeneration;
  }

  setConnected(connected: boolean, error: string | null = null): void {
    if (!connected) {
      for (const session of Object.values(this.value.sessions)) this.invalidateSecurity(session);
    }
    this.value.connected = connected;
    this.value.error = error;
    this.changed();
  }

  setDiagnostic(diagnostic: string): void {
    this.value.diagnostic = diagnostic;
    this.changed();
  }

  // A different model, removed/reloaded session, account or runtime gets a new
  // identity, including an A -> B -> A model switch while a request is pending.
  securityContextToken(sessionId: string): object | null {
    const session = this.value.sessions[sessionId];
    if (!session) return null;
    let token = this.securityContexts.get(session);
    if (!token) this.securityContexts.set(session, token = {});
    return token;
  }

  setSecurityVerification(sessionId: string, update: {
    pending: boolean; evidence?: SecurityEvidence | null; error?: string | null; status?: SecurityStatus;
  }): void {
    const session = this.value.sessions[sessionId];
    if (!session) return;
    session.securityVerificationPending = update.pending;
    session.securityVerificationError = update.error ?? null;
    if (update.evidence !== undefined) session.securityEvidence = update.evidence;
    if (update.status) session.security = update.status;
    this.changed();
  }

  private invalidateSecurity(session: ClientSessionState): void {
    this.securityContexts.delete(session);
    session.security = { state: "unverified" };
    session.securityEvidence = null;
    session.securityVerificationPending = false;
    session.securityVerificationError = null;
  }

  setAccount(account: AccountStatus): void {
    if (!this.applyAccount(account)) return;
    this.changed();
  }

  private applyAccount(account: AccountStatus): boolean {
    if (this.value.account && account.revision <= this.value.account.revision) return false;
    const previousAccountId = this.value.account?.account?.id ?? null;
    const nextAccountId = account.state === "valid" ? account.account?.id ?? null : null;
    if (previousAccountId !== nextAccountId) {
      // Account context is one atomic replacement. No thread, optimistic
      // timeline item, collection, preference, security state, or stale error
      // from the previous account survives this publication.
      this.value.sessions = {};
      this.textRevisions.clear();
      this.value.catalog = [];
      this.value.collections = { revision: 0, collections: [] };
      this.value.preferences = null;
      this.value.billing = null;
      this.value.error = null;
      this.accountContextGeneration += 1;
    }
    this.value.account = account;
    return true;
  }

  setCollections(collections: CollectionState): void {
    if (collections.revision >= this.value.collections.revision) this.value.collections = collections;
    this.changed();
  }

  setPreferences(preferences: ProfilePreferences): void {
    this.value.preferences = preferences;
    this.changed();
  }

  setBilling(status: BillingStatus): void {
    if (this.value.billing && status.revision <= this.value.billing.revision) return;
    if (this.value.billing && status.ledgerSequence < this.value.billing.ledgerSequence) return;
    this.value.billing = status;
    this.changed();
  }

  addSession(sessionId: string, cwd: string): ClientSessionState {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId, cwd));
    if (cwd) session.cwd = cwd;
    this.changed();
    return session;
  }

  removeSession(sessionId: string): void {
    delete this.value.sessions[sessionId];
    this.textRevisions.delete(sessionId);
    this.changed();
  }

  setCatalog(catalog: ThreadSummary[]): void {
    this.value.catalog = catalog;
    for (const thread of catalog) {
      if (this.value.sessions[thread.threadId]) this.applyThreadSummary(thread);
    }
    this.changed();
  }

  applyThreadRename(thread: ThreadSummary): void {
    const catalog = this.value.catalog.find((entry) => entry.threadId === thread.threadId);
    const latest = catalog && catalog.revision > thread.revision ? catalog : thread;
    if (catalog && latest === thread) Object.assign(catalog, thread);
    const session = this.value.sessions[thread.threadId];
    // A live delta may have advanced the session beyond the rename revision.
    // Update only its title, never roll back its stream/settings or load a chat.
    if (session) session.title = latest.title ?? null;
    this.changed();
  }

  replaceTimeline(pages: GetThreadTimelineResponse[]): boolean {
    const first = pages[0];
    if (!first) throw new Error("timeline replacement requires at least one page");
    const threadId = first.thread.threadId;
    const revision = first.thread.revision;
    const lastSequence = first.thread.lastTimelineSequence;
    if (pages.some((page) =>
      page.thread.threadId !== threadId
      || page.thread.revision !== revision
      || page.thread.lastTimelineSequence !== lastSequence
    )) {
      throw new Error("timeline changed while it was being paginated");
    }
    const existing = this.value.sessions[threadId];
    if (existing && revision < existing.threadRevision) {
      existing.needsResync = true;
      this.changed();
      return false;
    }
    const items = pages.flatMap((page) => page.items);
    let previousSequence = 0;
    const ids = new Set<string>();
    for (const item of items) {
      if (item.threadId !== threadId || item.sequence <= previousSequence || ids.has(item.id)) {
        throw new Error("timeline page order or identity is invalid");
      }
      previousSequence = item.sequence;
      ids.add(item.id);
    }
    if (items.at(-1)?.sequence !== lastSequence && lastSequence !== 0) {
      throw new Error("timeline replacement did not reach the authoritative tail");
    }
    const requestUsage = pages.flatMap((page) => page.requestUsage ?? []);
    const usageIds = new Set<string>();
    for (const usage of requestUsage) {
      if (!validRequestUsage(usage) || usageIds.has(usage.requestId)) {
        throw new Error("timeline accounting page identity is invalid");
      }
      usageIds.add(usage.requestId);
    }
    const session = this.applyThreadSummary(first.thread);
    session.activeTurnId = session.running ? first.activeTurnId ?? null : null;
    session.contextUsage = validContextUsage(first.contextUsage);
    session.requestUsage = requestUsage;
    session.desktopAgent = first.desktopAgent ?? null;
    session.timeline = items.map(fromDurable);
    this.textRevisions.set(threadId, revision);
    session.needsResync = false;
    this.changed();
    return true;
  }

  setDesktopAgent(response: ConfigureDesktopAgentResponse): void {
    const existing = this.value.sessions[response.thread.threadId];
    if (existing && response.thread.revision < existing.threadRevision) return;
    const session = this.applyThreadSummary(response.thread);
    session.desktopAgent = response.agent;
    session.currentModeId = response.thread.profile;
    this.changed();
  }

  setModes(sessionId: string, currentModeId: string, modes: SessionMode[]): void {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    session.currentModeId = currentModeId;
    session.modes = modes;
    this.changed();
  }

  setConfig(sessionId: string, options: SessionConfigOption[]): void {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    session.configOptions = options;
    const model = options.find((option) => option.id === "model")?.currentValue;
    const thinking = options.find((option) => option.id === "thinking")?.currentValue;
    if (model || thinking) {
      session.settings ??= { model: "", thinkingLevel: "medium", permissionProfile: "web" };
      if (model) {
        if (model !== session.settings.model) this.invalidateSecurity(session);
        session.settings.model = model;
      }
      if (thinking) session.settings.thinkingLevel = thinking;
    }
    this.changed();
  }

  setRunning(sessionId: string, running: boolean): void {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    session.running = running;
    if (!running) session.activeTurnId = null;
    this.changed();
  }

  addInteraction(interaction: PendingInteraction): void {
    if (!interaction.sessionId) return;
    const session = (this.value.sessions[interaction.sessionId] ??= emptySession(interaction.sessionId));
    session.interactions.push(interaction);
    this.changed();
  }

  removeInteraction(interactionId: string): void {
    for (const session of Object.values(this.value.sessions)) {
      session.interactions = session.interactions.filter((item) => item.id !== interactionId);
    }
    this.changed();
  }

  standardUpdate(params: unknown): string | null {
    if (this.value.account !== null && this.value.account.state !== "valid") {
      return null;
    }
    const record = object(params);
    if (!record || typeof record.sessionId !== "string") return null;
    const update = object(record.update);
    if (!update) return null;
    const session = (this.value.sessions[record.sessionId] ??= emptySession(record.sessionId));
    const metadata = deliveryMetadata(record, update);
    if (metadata) {
      if (metadata.threadRevision < session.threadRevision) return null;
      if (session.threadRevision > 0 && metadata.threadRevision > session.threadRevision + 1) {
        session.needsResync = true;
      }
      session.threadRevision = Math.max(session.threadRevision, metadata.threadRevision);
      session.lastTimelineSequence = Math.max(session.lastTimelineSequence, metadata.lastTimelineSequence);
      if (metadata.lastMessageAt !== undefined) session.lastMessageAt = metadata.lastMessageAt;
      if (metadata.lastUserMessageAt !== undefined) session.lastUserMessageAt = metadata.lastUserMessageAt;
    }

    const type = String(update.sessionUpdate ?? "");
    if (type === "user_message_chunk" || type === "agent_message_chunk" || type === "agent_thought_chunk") {
      if (metadata && metadata.threadRevision <= (this.textRevisions.get(record.sessionId) ?? -1)) {
        return session.needsResync ? record.sessionId : null;
      }
      const kind = type === "user_message_chunk" ? "user" : type === "agent_message_chunk" ? "assistant" : "reasoning";
      const text = textFromContent(update.content);
      const messageId = typeof update.messageId === "string"
        ? update.messageId
        : metadata?.timelineItemId ?? metadata?.clientItemId ?? crypto.randomUUID();
      const itemId = metadata?.timelineItemId ?? messageId;
      const turnId = messageTurnId(messageId, kind);
      // Native updates identify the exact durable segment. For standard ACP
      // peers without this metadata, only reuse a row AFTER the last tool:
      // the same turn can contain text -> tool -> text -> tool -> text.
      const recentItems = [...session.timeline].reverse();
      const boundary = recentItems.findIndex((item) => item.kind === "tool" || item.kind === "user");
      const segment = kind === "user" || boundary < 0 ? recentItems : recentItems.slice(0, boundary);
      const existing = metadata?.timelineItemId
        ? session.timeline.find((item) => item.id === itemId)
        : segment.find((item) => item.id === itemId
          || (turnId && kind !== "user" && item.kind === kind && item.turnId === turnId));
      if (existing) {
        existing.text += text;
        existing.status = kind === "user" ? "completed" : "streaming";
        if (kind !== "user") existing.terminalVerified = false;
      }
      else session.timeline.push({
        id: session.timeline.some((item) => item.id === itemId) ? crypto.randomUUID() : itemId,
        turnId,
        clientItemId: metadata?.clientItemId ?? undefined,
        kind,
        text,
        status: kind === "user" ? "completed" : "streaming",
        ...(kind === "user" && Array.isArray(object(update._meta)?.axiomAttachments)
          ? { raw: { metadata: { attachments: object(update._meta)!.axiomAttachments } } } : {}),
      });
      if (metadata) this.textRevisions.set(record.sessionId, metadata.threadRevision);
    } else if (type === "tool_call" || type === "tool_call_update") {
      const callId = String(update.toolCallId ?? update.id ?? metadata?.timelineItemId ?? crypto.randomUUID());
      const existing = session.timeline.find((item) => item.id === callId || item.tool?.callId === callId);
      if (existing) {
        existing.text = typeof update.title === "string" ? update.title : existing.text;
        existing.status = typeof update.status === "string" ? update.status : existing.status;
        existing.raw = { ...(object(existing.raw) ?? {}), ...update };
        existing.tool = mergeTool(existing.tool, callId, update);
      } else {
        const tool = mergeTool(undefined, callId, update);
        const turnId = [...session.timeline].reverse().find((item) => item.turnId)?.turnId;
        session.timeline.push({ id: callId, turnId, kind: "tool", text: tool.title, status: tool.status, raw: update, tool });
      }
    } else if (type === "plan") {
      const itemId = metadata?.timelineItemId ?? String(update.id ?? crypto.randomUUID());
      const existing = session.timeline.find((item) => item.id === itemId);
      if (existing) {
        existing.text = typeof update.content === "string" ? update.content : existing.text;
        existing.raw = update;
      } else {
        session.timeline.push({ id: itemId, kind: "plan", text: typeof update.content === "string" ? update.content : "Plan updated", raw: update });
      }
    } else if (type === "current_mode_update" && typeof update.currentModeId === "string") {
      session.currentModeId = update.currentModeId;
    } else if (type === "config_option_update" && Array.isArray(update.configOptions)) {
      this.setConfig(record.sessionId, update.configOptions as SessionConfigOption[]);
      return session.needsResync ? record.sessionId : null;
    } else if (type === "session_info_update" && typeof update.title === "string") {
      const summary = this.value.catalog.find((thread) => thread.threadId === record.sessionId);
      if (!metadata || !summary || metadata.threadRevision >= summary.revision) {
        session.title = update.title;
        if (summary) {
          summary.title = update.title;
          if (metadata) summary.revision = Math.max(summary.revision, metadata.threadRevision);
        }
      }
    }
    this.changed();
    return session.needsResync ? record.sessionId : null;
  }

  extensionEvent(notification: EventNotification): ExtensionEventResult | null {
    const previousRuntime = this.value.runtimeInstanceId;
    if (
      previousRuntime === notification.runtimeInstanceId
      && notification.sequence <= this.value.lastSequence
    ) {
      return null;
    }
    const runtimeChanged = previousRuntime !== null && previousRuntime !== notification.runtimeInstanceId;
    const sequenceGap = previousRuntime === notification.runtimeInstanceId
      && notification.sequence > this.value.lastSequence + 1;
    const fullResync = runtimeChanged || sequenceGap;
    if (runtimeChanged) {
      this.value.lastSequence = 0;
      // Account revisions are monotonic only within one agent runtime.
      this.value.account = null;
      this.value.sessions = {};
      this.textRevisions.clear();
      this.value.catalog = [];
      this.value.collections = { revision: 0, collections: [] };
      this.value.preferences = null;
      this.value.billing = null;
      this.value.error = null;
      this.accountContextGeneration += 1;
    } else if (sequenceGap) {
      for (const session of Object.values(this.value.sessions)) session.needsResync = true;
    }
    this.value.runtimeInstanceId = notification.runtimeInstanceId;
    this.value.lastSequence = Math.max(this.value.lastSequence, notification.sequence);
    this.applyActivity(notification.sessionId ?? null, notification.event);
    const session = notification.sessionId ? this.value.sessions[notification.sessionId] : undefined;
    if (session && notification.threadRevision !== undefined && notification.threadRevision !== null) {
      if (session.threadRevision > 0 && notification.threadRevision > session.threadRevision + 1) session.needsResync = true;
      session.threadRevision = Math.max(session.threadRevision, notification.threadRevision);
      session.lastTimelineSequence = Math.max(session.lastTimelineSequence, notification.lastTimelineSequence ?? 0);
    }
    this.changed();
    return {
      threadId: session?.needsResync ? notification.sessionId ?? null : null,
      fullResync,
    };
  }

  private applyThreadSummary(thread: ThreadSummary): ClientSessionState {
    const session = (this.value.sessions[thread.threadId] ??= emptySession(thread.threadId, thread.cwd));
    if (thread.revision < session.threadRevision) return session;
    session.title = thread.title ?? null;
    session.cwd = thread.cwd;
    if (session.settings?.model && session.settings.model !== thread.selectedModel) this.invalidateSecurity(session);
    session.settings = settingsFromThread(thread);
    session.running = !["ready", "closed"].includes(thread.lifecycle);
    if (!session.running) session.activeTurnId = null;
    session.threadRevision = thread.revision;
    session.lastTimelineSequence = thread.lastTimelineSequence;
    session.lastMessageAt = thread.lastMessageAt ?? null;
    session.lastUserMessageAt = thread.lastUserMessageAt ?? null;
    return session;
  }

  private applyActivity(sessionId: string | null, event: ActivityEvent): void {
    if (event.kind === "account_changed") {
      this.applyAccount(event.status);
    }
    else if (this.value.account !== null && this.value.account.state !== "valid") {
      // AxiomCLI publishes a switching state before it drains old-account
      // work. Ignore any already-buffered account projection until the final
      // valid account status establishes the new context.
      return;
    }
    else if (event.kind === "active_turn_changed" && sessionId) {
      const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
      session.activeTurnId = event.turnId ?? null;
    }
    else if (event.kind === "collections_changed") {
      if (event.state.revision >= this.value.collections.revision) this.value.collections = event.state;
    } else if (event.kind === "profile_preferences_changed") this.value.preferences = event.preferences;
    else if (event.kind === "billing_changed") {
      if (
        !this.value.billing
        || (
          event.status.revision > this.value.billing.revision
          && event.status.ledgerSequence >= this.value.billing.ledgerSequence
        )
      ) {
        this.value.billing = event.status;
      }
    }
    else if (event.kind === "security_changed" && sessionId) {
      (this.value.sessions[sessionId] ??= emptySession(sessionId)).security = event.status;
    }
    else if (event.kind === "request_usage_changed" && sessionId && validRequestUsage(event.usage)) {
      const session = this.value.sessions[sessionId] ??= emptySession(sessionId);
      const index = (session.requestUsage ??= []).findIndex((item) => item.requestId === event.usage.requestId);
      if (index < 0) session.requestUsage.push(event.usage);
      else session.requestUsage[index] = event.usage;
      if (event.usage.responseVerified) session.needsResync = true;
    }
    else if (event.kind === "context_usage_changed" && sessionId) {
      (this.value.sessions[sessionId] ??= emptySession(sessionId)).contextUsage = validContextUsage(event.usage);
    }
  }

  private changed(): void {
    this.emit("change", this.snapshot());
  }
}

function validRequestUsage(usage: RequestUsage): boolean {
  const decimal = (value: unknown) => value == null || (typeof value === "string" && /^\d{1,20}$/.test(value) && BigInt(value) <= 18446744073709551615n);
  return !!usage && typeof usage.requestId === "string" && /^[a-f0-9]{32}$/.test(usage.requestId)
    && [usage.inputTokens, usage.cachedInputTokens, usage.outputTokens, usage.reasoningTokens, usage.costMicrousd].every(decimal)
    && !(usage.cachedInputTokens != null && usage.inputTokens != null && BigInt(usage.cachedInputTokens) > BigInt(usage.inputTokens))
    && !(usage.reasoningTokens != null && usage.outputTokens != null && BigInt(usage.reasoningTokens) > BigInt(usage.outputTokens));
}

function validContextUsage(usage: ContextUsage | null | undefined): ContextUsage | null {
  if (!usage || !Number.isSafeInteger(usage.inputTokens) || usage.inputTokens < 0
    || !Number.isSafeInteger(usage.outputTokens) || usage.outputTokens < 0
    || !Number.isSafeInteger(usage.inputTokens + usage.outputTokens)
    || typeof usage.modelId !== "string" || !usage.modelId.trim()
    || typeof usage.reportedAt !== "string" || !Number.isFinite(Date.parse(usage.reportedAt))) return null;
  const validLimit = (value: number | null | undefined) => value == null
    || (Number.isSafeInteger(value) && value > 0);
  if (!validLimit(usage.contextWindowTokens) || !validLimit(usage.autoCompactThresholdTokens)
    || (usage.contextWindowTokens != null && usage.autoCompactThresholdTokens != null
      && usage.autoCompactThresholdTokens > usage.contextWindowTokens)) return null;
  return usage;
}

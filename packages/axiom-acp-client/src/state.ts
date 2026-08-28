import { EventEmitter } from "node:events";
import type {
  AccountStatus,
  ActivityEvent,
  CollectionState,
  DeliveryMetadata,
  EventNotification,
  GetThreadTimelineResponse,
  ProfilePreferences,
  SecurityStatus,
  ThreadSummary,
  TimelineItem as DurableTimelineItem,
} from "./generated/protocol.js";
import type { PendingInteraction, SessionConfigOption, SessionMode } from "./types.js";

export type ToolActivity = {
  callId: string;
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
  modes: SessionMode[];
  currentModeId: string | null;
  configOptions: SessionConfigOption[];
  timeline: ClientTimelineItem[];
  interactions: PendingInteraction[];
  running: boolean;
  needsResync: boolean;
  threadRevision: number;
  lastTimelineSequence: number;
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
    modes: [],
    currentModeId: null,
    configOptions: [],
    timeline: [],
    interactions: [],
    running: false,
    needsResync: false,
    threadRevision: 0,
    lastTimelineSequence: 0,
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
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "assistant", text: item.content, status: item.status, terminalVerified, raw: item };
    case "reasoning":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "reasoning", text: item.content, status: item.status, terminalVerified, raw: item };
    case "tool_call": {
      const tool = durableTool(item);
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "tool", text: tool.title, status: item.status, raw: item, tool };
    }
    case "plan":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: "plan", text: item.content || "Plan updated", status: item.status, raw: item };
    case "notice":
      return { id: item.id, turnId: item.turnId ?? undefined, sequence: item.sequence, kind: item.status === "failed" ? "error" : "activity", text: item.content, status: item.status, raw: item };
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
  private value: ClientState = {
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

  snapshot(): ClientState {
    return structuredClone(this.value);
  }

  setConnected(connected: boolean, error: string | null = null): void {
    this.value.connected = connected;
    this.value.error = error;
    this.changed();
  }

  setDiagnostic(diagnostic: string): void {
    this.value.diagnostic = diagnostic;
    this.changed();
  }

  setAccount(account: AccountStatus): void {
    this.value.account = account;
    this.changed();
  }

  setCollections(collections: CollectionState): void {
    if (collections.revision >= this.value.collections.revision) this.value.collections = collections;
    this.changed();
  }

  setPreferences(preferences: ProfilePreferences): void {
    this.value.preferences = preferences;
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
    this.changed();
  }

  setCatalog(catalog: ThreadSummary[]): void {
    this.value.catalog = catalog;
    for (const thread of catalog) {
      if (this.value.sessions[thread.threadId]) this.applyThreadSummary(thread);
    }
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
    const session = this.applyThreadSummary(first.thread);
    session.timeline = items.map(fromDurable);
    session.needsResync = false;
    this.changed();
    return true;
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
      if (model) session.settings.model = model;
      if (thinking) session.settings.thinkingLevel = thinking;
    }
    this.changed();
  }

  setRunning(sessionId: string, running: boolean): void {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    session.running = running;
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
    }

    const type = String(update.sessionUpdate ?? "");
    if (type === "user_message_chunk" || type === "agent_message_chunk" || type === "agent_thought_chunk") {
      const kind = type === "user_message_chunk" ? "user" : type === "agent_message_chunk" ? "assistant" : "reasoning";
      const text = textFromContent(update.content);
      const itemId = typeof update.messageId === "string"
        ? update.messageId
        : metadata?.timelineItemId ?? metadata?.clientItemId ?? crypto.randomUUID();
      const existing = session.timeline.find((item) => item.id === itemId);
      if (existing) existing.text += text;
      else session.timeline.push({
        id: itemId,
        turnId: messageTurnId(itemId, kind),
        clientItemId: metadata?.clientItemId ?? undefined,
        kind,
        text,
        status: kind === "user" ? "completed" : "streaming",
      });
    } else if (type === "tool_call" || type === "tool_call_update") {
      const callId = String(update.toolCallId ?? update.id ?? metadata?.timelineItemId ?? crypto.randomUUID());
      const existing = session.timeline.find((item) => item.id === callId);
      if (existing) {
        existing.text = typeof update.title === "string" ? update.title : existing.text;
        existing.status = typeof update.status === "string" ? update.status : existing.status;
        existing.raw = { ...(object(existing.raw) ?? {}), ...update };
        existing.tool = mergeTool(existing.tool, callId, update);
      } else {
        const tool = mergeTool(undefined, callId, update);
        session.timeline.push({ id: callId, kind: "tool", text: tool.title, status: tool.status, raw: update, tool });
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
      session.title = update.title;
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
      for (const session of Object.values(this.value.sessions)) session.needsResync = true;
      this.value.lastSequence = 0;
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
    session.settings = settingsFromThread(thread);
    session.running = !["ready", "closed"].includes(thread.lifecycle);
    session.threadRevision = thread.revision;
    session.lastTimelineSequence = thread.lastTimelineSequence;
    return session;
  }

  private applyActivity(sessionId: string | null, event: ActivityEvent): void {
    if (event.kind === "account_changed") this.value.account = event.status;
    else if (event.kind === "collections_changed") {
      if (event.state.revision >= this.value.collections.revision) this.value.collections = event.state;
    } else if (event.kind === "profile_preferences_changed") this.value.preferences = event.preferences;
    else if (event.kind === "security_changed" && sessionId) {
      (this.value.sessions[sessionId] ??= emptySession(sessionId)).security = event.status;
    }
  }

  private changed(): void {
    this.emit("change", this.snapshot());
  }
}

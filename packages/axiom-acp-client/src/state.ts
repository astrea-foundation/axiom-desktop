import { EventEmitter } from "node:events";
import type {
  AccountStatus,
  ActivityEvent,
  EventNotification,
  SecurityStatus,
  SessionSettings,
  SessionSnapshot,
  SessionSummary,
} from "./generated/protocol.js";
import type { PendingInteraction, SessionConfigOption, SessionMode } from "./types.js";

export type TimelineItem = {
  id: string;
  kind: "user" | "assistant" | "reasoning" | "tool" | "plan" | "activity" | "error";
  text: string;
  status?: string;
  raw?: unknown;
};

export interface ClientSessionState {
  sessionId: string;
  title: string | null;
  cwd: string;
  settings: SessionSettings | null;
  security: SecurityStatus | null;
  snapshot: SessionSnapshot | null;
  modes: SessionMode[];
  currentModeId: string | null;
  configOptions: SessionConfigOption[];
  timeline: TimelineItem[];
  interactions: PendingInteraction[];
  running: boolean;
  needsResync: boolean;
}

export interface ClientState {
  connected: boolean;
  runtimeInstanceId: string | null;
  lastSequence: number;
  sessions: Record<string, ClientSessionState>;
  catalog: SessionSummary[];
  account: AccountStatus | null;
  diagnostic: string;
  error: string | null;
}

function emptySession(sessionId: string, cwd = ""): ClientSessionState {
  return {
    sessionId,
    title: null,
    cwd,
    settings: null,
    security: null,
    snapshot: null,
    modes: [],
    currentModeId: null,
    configOptions: [],
    timeline: [],
    interactions: [],
    running: false,
    needsResync: false,
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

export class AxiomStateStore extends EventEmitter {
  private value: ClientState = {
    connected: false,
    runtimeInstanceId: null,
    lastSequence: 0,
    sessions: {},
    catalog: [],
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

  setCatalog(catalog: SessionSummary[]): void {
    this.value.catalog = catalog;
    this.changed();
  }

  setSnapshot(snapshot: SessionSnapshot): void {
    const session = (this.value.sessions[snapshot.sessionId] ??= emptySession(
      snapshot.sessionId,
      snapshot.cwd,
    ));
    session.snapshot = snapshot;
    session.title = snapshot.title ?? null;
    session.cwd = snapshot.cwd;
    session.settings = snapshot.settings;
    session.security = snapshot.security;
    session.running = snapshot.lifecycle !== "ready" && snapshot.lifecycle !== "closed";
    session.needsResync = false;
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
    this.changed();
  }

  setRunning(sessionId: string, running: boolean): void {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    session.running = running;
    if (!running) {
      for (const item of session.timeline) {
        if (item.status === "streaming") item.status = "complete";
      }
    }
    this.changed();
  }

  addUserMessage(sessionId: string, text: string): void {
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    session.timeline.push({ id: crypto.randomUUID(), kind: "user", text });
    this.changed();
  }

  addInteraction(interaction: PendingInteraction): void {
    if (!interaction.sessionId) return;
    const session = (this.value.sessions[interaction.sessionId] ??= emptySession(
      interaction.sessionId,
    ));
    session.interactions.push(interaction);
    this.changed();
  }

  removeInteraction(interactionId: string): void {
    for (const session of Object.values(this.value.sessions)) {
      session.interactions = session.interactions.filter((item) => item.id !== interactionId);
    }
    this.changed();
  }

  standardUpdate(params: unknown): void {
    if (!params || typeof params !== "object") return;
    const record = params as Record<string, unknown>;
    if (typeof record.sessionId !== "string" || !record.update || typeof record.update !== "object") {
      return;
    }
    const session = (this.value.sessions[record.sessionId] ??= emptySession(record.sessionId));
    const update = record.update as Record<string, unknown>;
    const type = String(update.sessionUpdate ?? "");
    if (type === "agent_message_chunk" || type === "agent_thought_chunk") {
      const kind = type === "agent_message_chunk" ? "assistant" : "reasoning";
      const text = textFromContent(update.content);
      const previous = session.timeline.at(-1);
      if (previous?.kind === kind && previous.status === "streaming") previous.text += text;
      else session.timeline.push({ id: crypto.randomUUID(), kind, text, status: "streaming" });
    } else if (type === "tool_call" || type === "tool_call_update") {
      const callId = String(update.toolCallId ?? update.id ?? crypto.randomUUID());
      const text = String(update.title ?? update.name ?? "Tool activity");
      const existing = session.timeline.find((item) => item.id === callId);
      if (existing) {
        existing.status = typeof update.status === "string" ? update.status : existing.status;
        existing.raw = update;
      } else {
        session.timeline.push({ id: callId, kind: "tool", text, status: String(update.status ?? "pending"), raw: update });
      }
    } else if (type === "plan") {
      session.timeline.push({ id: crypto.randomUUID(), kind: "plan", text: "Plan updated", raw: update });
    } else if (type === "current_mode_update" && typeof update.currentModeId === "string") {
      session.currentModeId = update.currentModeId;
    } else if (type === "config_option_update" && Array.isArray(update.configOptions)) {
      session.configOptions = update.configOptions as SessionConfigOption[];
    } else if (type === "session_info_update" && typeof update.title === "string") {
      session.title = update.title;
    }
    this.changed();
  }

  extensionEvent(notification: EventNotification): string | null {
    if (
      this.value.runtimeInstanceId === notification.runtimeInstanceId &&
      notification.sequence > this.value.lastSequence + 1
    ) {
      if (notification.sessionId) {
        const session = (this.value.sessions[notification.sessionId] ??= emptySession(
          notification.sessionId,
        ));
        session.needsResync = true;
      }
    }
    if (this.value.runtimeInstanceId !== notification.runtimeInstanceId) {
      this.value.runtimeInstanceId = notification.runtimeInstanceId;
      this.value.lastSequence = 0;
      for (const session of Object.values(this.value.sessions)) session.needsResync = true;
    }
    this.value.lastSequence = Math.max(this.value.lastSequence, notification.sequence);
    this.applyActivity(notification.sessionId ?? null, notification.event);
    this.changed();
    return notification.sessionId ?? null;
  }

  private applyActivity(sessionId: string | null, event: ActivityEvent): void {
    if (event.kind === "account_changed") {
      this.value.account = event.status;
      return;
    }
    if (!sessionId) return;
    const session = (this.value.sessions[sessionId] ??= emptySession(sessionId));
    if (event.kind === "session_state") {
      session.snapshot = event.snapshot;
      session.settings = event.snapshot.settings;
      session.security = event.snapshot.security;
    } else if (event.kind === "title_changed") session.title = event.title;
    else if (event.kind === "settings_changed") session.settings = event.settings;
    else if (event.kind === "security_changed") session.security = event.status;
    else {
      session.timeline.push({
        id: crypto.randomUUID(),
        kind: event.kind === "error" ? "error" : "activity",
        text:
          "message" in event && typeof event.message === "string"
            ? event.message
            : event.kind.replaceAll("_", " "),
        raw: event,
      });
    }
  }

  private changed(): void {
    this.emit("change", this.snapshot());
  }
}

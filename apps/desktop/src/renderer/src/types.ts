import type { PromptAttachment } from "@axiom/axiom-acp-client";

export type ReasoningEffort = "provider_default" | "enabled" | "disabled" | "minimal" | "low" | "medium" | "high" | "xhigh";

export type ChatRole = "user" | "assistant";

export type MessageStatus = "complete" | "streaming" | "pending" | "failed" | "interrupted" | "cancelled";

export type MainSurface = "welcome" | "thread" | "proxy";

export interface ProviderModel {
  id: string;
  label: string;
  shortLabel: string;
  providerId: string;
  providerLabel: string;
  model: string;
  supportedReasoningEfforts: ReasoningEffort[];
  inputPriceMicrousdPerMillionTokens: number | null;
  outputPriceMicrousdPerMillionTokens: number | null;
  contextWindowTokens?: number;
  supportsImages?: boolean;
  fileMimeTypes?: string[];
  autoCompactThresholdTokens?: number;
}

export interface Message {
  id: string;
  role: ChatRole;
  content: string;
  status: MessageStatus;
  reasoningContent?: string;
  reasoningDurationMs?: number;
  terminalVerified?: boolean;
  finishReason?: string;
  /** More text or tools follow in this turn; this is not its final bubble. */
  continues?: boolean;
}

export interface PendingUserMessage {
  id: string;
  sessionId: string;
  text: string;
  attachments?: PromptAttachment[];
}

export type ThreadStatus = "idle" | "unread" | "working";

export interface Thread {
  id: string;
  title: string;
  folderId: string | null;
  updatedAt: string;
  lastMessageAt?: string | null;
  lastUserMessageAt?: string | null;
  status: ThreadStatus;
  messages: Message[];
}

export interface Folder {
  id: string;
  name: string;
  collapsed: boolean;
}

export interface DemoApi {
  setTheme: (theme: "light" | "dark" | "system") => void;
  showWelcome: () => void;
  openModelPicker: () => void;
  showProxy: () => void;
  showFolder: () => void;
  showThread: (id?: string) => void;
  showSettings: (open: boolean) => void;
  setSidebar: (open: boolean) => void;
  showSignIn: (open?: boolean) => void;
}

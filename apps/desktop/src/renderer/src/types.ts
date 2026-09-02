export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh";

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
}

export interface Message {
  id: string;
  role: ChatRole;
  content: string;
  status: MessageStatus;
  reasoningContent?: string;
  reasoningDurationMs?: number;
  terminalVerified?: boolean;
}

export interface PendingUserMessage {
  id: string;
  sessionId: string;
  text: string;
}

export interface Thread {
  id: string;
  title: string;
  folderId: string | null;
  updatedAt: string;
  messages: Message[];
}

export interface Folder {
  id: string;
  name: string;
  collapsed: boolean;
}

export interface DemoApi {
  showWelcome: () => void;
  openModelPicker: () => void;
  showProxy: () => void;
  showFolder: () => void;
  showThread: (id?: string) => void;
  showSignIn: () => void;
}

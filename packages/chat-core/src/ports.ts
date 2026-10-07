import type { ClientSessionState, ClientState, PromptAttachment } from "./viewTypes";
import type { QueueApi } from "./messageQueue";

export interface HostCapabilities {
  agent: boolean;
  localMcp: boolean;
  webSearch: boolean;
  originalUploads: boolean;
  vault: boolean;
}
export interface ConversationStore<T> {
  load(): Promise<T | null>;
  save(value: T): Promise<void>;
  lock(): void;
}
export interface ChatRuntime extends QueueApi {
  snapshot(): ClientState;
  subscribe(changed: () => void): () => void;
  revise(threadId: string, userItemId: string, text: string): Promise<void>;
  session(threadId: string): ClientSessionState | undefined;
  attachments(threadId: string, userItemId: string): PromptAttachment[];
  dispose(): void;
}

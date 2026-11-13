/** @jsxRuntime automatic */
import { createContext, useContext, type ReactNode } from 'react';
import type { PromptAttachment, PermissionOutcome, ElicitationOutcome, UsageSummary, UsageSummaryRequest } from '@axiom/chat-core/viewTypes';
import type { CreditUsageSummary } from '@axiom/chat-core/creditUsage';

export interface ChatHost {
  chrome?: { isMac: boolean; fullScreen: boolean; trafficLightInsetClass: string };
  getAttachments?(threadId: string, userItemId: string): Promise<{ attachments: PromptAttachment[] }>;
  regenerateImage?(threadId: string, callId: string): Promise<{ callId: string }>;
  loadGeneratedImage?(assetId: string): Promise<{ imageUrl: string; width: number; height: number }>;
  releaseGeneratedImage?(assetId: string): void;
  usageSummary?(accountId: string, request: Pick<UsageSummaryRequest, 'period' | 'timezone'>): Promise<{ summary: UsageSummary }>;
  creditUsageSummary?(accountId: string, request: Pick<UsageSummaryRequest, 'period' | 'timezone'>): Promise<CreditUsageSummary | null>;
  resolvePermission?(interactionId: string, outcome: PermissionOutcome): Promise<unknown>;
  resolveElicitation?(interactionId: string, outcome: ElicitationOutcome): Promise<unknown>;
}
const Context = createContext<ChatHost>({});
export function ChatHostProvider({ host, children }: { host: ChatHost; children: ReactNode }) {
  return <Context.Provider value={host}>{children}</Context.Provider>;
}
export function useChatHost(): ChatHost { return useContext(Context); }

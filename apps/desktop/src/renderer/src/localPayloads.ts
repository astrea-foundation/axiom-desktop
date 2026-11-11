import { LocalPayloadStore } from "@axiom/chat-core/localPayloads";
import { NativePersistence } from "./nativePersistence";
export type { PromptPayload, PayloadStore } from "@axiom/chat-core/localPayloads";

export const localPayloads = new NativePersistence(
  () => window.axiomDesktop?.agent,
  new LocalPayloadStore(),
  () => { try { return window.localStorage; } catch { return null; } },
);

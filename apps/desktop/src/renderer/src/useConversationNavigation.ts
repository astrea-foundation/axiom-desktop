import { useCallback, useRef, useState } from "react";
import type { MainSurface } from "./types";
import type { PromptAttachment } from "@axiom/axiom-acp-client";

export function useConversationNavigation() {
  const [surface, setSurface] = useState<MainSurface>("welcome");
  const [activeThreadId, setActiveThreadId] = useState<string | null>(null);
  const activeThreadIdRef = useRef(activeThreadId);
  const navigationRevision = useRef(0);
  const [threadPreparation, setThreadPreparation] = useState<{ id: string; text: string; attachments: PromptAttachment[]; accountId: string | null; runtimeId: string | null; } | null>(null);
  const cancelThreadPreparation = useCallback(() => {
    navigationRevision.current += 1;
    setThreadPreparation(null);
  }, []);

  return { surface, setSurface, activeThreadId, setActiveThreadId, activeThreadIdRef, navigationRevision, threadPreparation, setThreadPreparation, cancelThreadPreparation };
}

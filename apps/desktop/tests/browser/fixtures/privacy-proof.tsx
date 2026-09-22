import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import { ChatView } from "../../../src/renderer/src/components/ChatView";
import { securityEvidence } from "../../../../../fixtures/desktop-security.mts";
import type { ProviderModel } from "../../../src/renderer/src/types";
import "../../../src/renderer/src/index.css";

const model: ProviderModel = {
  id: "test-model", label: "DeepSeek V4 Flash", shortLabel: "DeepSeek V4 Flash", providerId: "near", providerLabel: "NEAR",
  model: "test-model", supportedReasoningEfforts: [], inputPriceMicrousdPerMillionTokens: null, outputPriceMicrousdPerMillionTokens: null,
};
function ProofHarness() {
  const [session, setSession] = useState<ClientSessionState>({
    sessionId: "thread", title: "Private conversation", cwd: "", settings: { model: model.id, thinkingLevel: "medium", permissionProfile: "web" },
    security: { state: "verified" }, securityEvidence: securityEvidence(),
    modes: [], currentModeId: null, configOptions: [], interactions: [], running: false,
    needsResync: false, threadRevision: 0, lastTimelineSequence: 0,
    timeline: [{ id: "question", kind: "user", text: "A quiet place to think.", status: "completed" }],
  });
  const [web, setWeb] = useState(false);
  const [scope, setScope] = useState("account-one:runtime-one");
  const [selectedModel, setModel] = useState(model.id);
  const [draft, setDraft] = useState("");
  const [disabledReason, setDisabledReason] = useState<string | undefined>();
  const [showChat, setShowChat] = useState(true);
  const requests = useRef(0);
  const acceptances = useRef(0);
  const acceptOutdated = async () => {
    acceptances.current++;
    const evidence = securityEvidence();
    evidence.status.state = "degraded";
    evidence.attestationProtocol = "near-tdx-nvidia-v2";
    evidence.e2eeProtocol = "near-v3";
    evidence.checks = evidence.checks.map((check) => check.name === "Intel TDX" ? { ...check, passed: false, detail: "OutOfDate" } : check);
    setSession((current) => ({ ...current, security: { state: "degraded" }, securityEvidence: evidence }));
  };
  const pending = useRef<{ resolve: () => void; reject: (error: Error) => void } | null>(null);
  const verify = () => {
    requests.current++;
    setSession((current) => ({ ...current, security: { state: "verifying" }, securityVerificationPending: true }));
    return new Promise<void>((resolve, reject) => { pending.current = { resolve, reject }; });
  };
  Object.assign(window, { __privacyTest: {
    calls: () => requests.current,
    acceptances: () => acceptances.current,
    update: (changes: Partial<ClientSessionState>) => setSession((current) => ({ ...current, ...changes })),
    setWeb, setScope, setModel, setDisabledReason, setShowChat,
    finish: (fail = false, keepEvidence = false) => {
      setSession((current) => ({ ...current,
        security: { state: fail ? "failed" : "verified", detail: fail ? "The supplied quote did not verify." : null },
        securityVerificationPending: false,
        securityEvidence: fail || keepEvidence ? current.securityEvidence : { ...securityEvidence(), modelId: selectedModel, verifiedAtUnixSeconds: Math.floor(Date.now() / 1000) },
      }));
      if (fail) pending.current?.reject(new Error("Could not verify")); else pending.current?.resolve();
      pending.current = null;
    },
  } });
  return <div style={{ height: "100vh" }} className="bg-[var(--color-bg-base)] p-4">
    <main className="relative h-full overflow-hidden rounded-[22px] bg-[var(--surface-stage)] shadow-glass-card">
      <header className="absolute inset-x-0 top-0 z-10 flex h-12 items-center justify-center text-[13px] text-[var(--color-text-secondary)]">Private conversation</header>
      {showChat ? <ChatView session={session} draft={draft} onDraftChange={setDraft} onSubmit={() => {}} onStop={() => {}}
        isStreaming={session.running} models={[model]} selectedModelId={selectedModel} modelStatusLabel="" onModelChange={setModel}
        requiresSignIn={false} onSignIn={() => {}} reasoningEffort="medium" onReasoningEffortChange={() => {}}
        onVerifySecurity={verify} securityScope={scope} securityRefreshDisabledReason={disabledReason}
        onAcceptOutdatedTee={acceptOutdated}
        webEnabled={web} webDisabled={false} onWebToggle={() => setWeb((value) => !value)} /> : null}
    </main>
  </div>;
}
createRoot(document.getElementById("root")!).render(<ProofHarness />);

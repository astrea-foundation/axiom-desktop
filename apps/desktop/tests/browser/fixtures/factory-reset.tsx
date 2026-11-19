import { createRoot } from "react-dom/client";
import { SettingsSheet } from "../../../src/renderer/src/components/SettingsSheet";
import type { AccountStatus } from "@axiom/axiom-acp-client";
import type { SettingsCategory } from "../../../src/renderer/src/components/SettingsPanel";
import "../../../src/renderer/src/index.css";

const params = new URLSearchParams(location.search);
const state = params.get("state") ?? "disconnected";
const actions: string[] = [];
Object.assign(window, { __accountActions: actions });
const account: AccountStatus | null = state === "valid" ? {revision: 1, state: "valid", account: {
  id: "account-fixture", displayName: params.has("long") ? "Alexandra".repeat(8) : "Alex Morgan",
  verifiedEmail: params.has("long") ? "alexandra.".repeat(12) + "@example.com" : "alex@example.com",
  linkedMethods: ["google", "passkey"],
}} : state === "expired" ? {revision: 1, state: "expired"}
  : state === "starting" ? {revision: 1, state: "starting", preparationPhase: "opening_chats"}
    : state === "signed_out" ? {revision: 1, state: "signed_out"} : null;

let chatCount = Number(params.get("chats") ?? "3"), clearCalls = 0, clearFails = false;
let finishClear: (() => void) | null = null;
Object.assign(window, { __clearTest: { calls: () => clearCalls, fail: () => { clearFails = true; }, finish: () => finishClear?.() } });

function render() {
  root.render(<SettingsSheet initialCategory={(params.get("category") ?? "chats") as SettingsCategory} connected={state !== "disconnected"} account={account}
    chats={{ count: chatCount, unavailableReason: state === "valid" ? undefined : "Sign in and connect to clear your chats.", onClear: async () => {
      clearCalls++;
      if (clearFails) { clearFails = false; throw new Error("The desktop service is unavailable. Please try again."); }
      await new Promise<void>((resolve) => { finishClear = resolve; });
      const deleted = chatCount; chatCount = 0; render(); return deleted;
    } }}
    onClose={() => {}} onRefreshAccount={() => {actions.push("refresh");}} onLogin={() => {actions.push("login");}}
    onLogout={() => {actions.push("logout");}} onManageAccount={() => {actions.push("manage");}} />);
}
const root = createRoot(document.getElementById("root")!);
render();

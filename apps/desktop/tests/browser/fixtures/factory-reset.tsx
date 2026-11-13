import { createRoot } from "react-dom/client";
import { SettingsSheet } from "../../../src/renderer/src/components/SettingsSheet";
import type { AccountStatus } from "@axiom/axiom-acp-client";
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

createRoot(document.getElementById("root")!).render(<SettingsSheet initialCategory="account" connected={state !== "disconnected"} account={account}
  onClose={() => {}} onRefreshAccount={() => {actions.push("refresh");}} onLogin={() => {actions.push("login");}}
  onLogout={() => {actions.push("logout");}} onManageAccount={() => {actions.push("manage");}} />);

import { createRoot } from "react-dom/client";
import { SettingsSheet } from "../../../src/renderer/src/components/SettingsSheet";
import "../../../src/renderer/src/index.css";

createRoot(document.getElementById("root")!).render(<SettingsSheet initialCategory="account" connected={false} account={null}
  onClose={() => {}} onRefreshAccount={() => {}} onLogin={() => {}} onLogout={() => {}} onManageAccount={() => {}} />);

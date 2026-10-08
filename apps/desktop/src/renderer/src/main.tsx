import { StrictMode, useMemo } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "katex/dist/katex.min.css";
import "./index.css";
import { ChatHostProvider, type ChatHost } from "@axiom/chat-ui";
import { isMac, MAC_TRAFFIC_LIGHT_INSET_CLASS, useFullScreen } from "./lib/window-chrome";

function DesktopHost() {
  const fullScreen = useFullScreen();
  const host = useMemo<ChatHost>(() => ({
    chrome: { isMac, fullScreen, trafficLightInsetClass: MAC_TRAFFIC_LIGHT_INSET_CLASS },
    getAttachments: async (thread, item) => {
      const api = window.axiomDesktop?.agent;
      if (!api) throw new Error("Desktop is unavailable");
      return api.getAttachments(thread, item);
    },
    usageSummary: async (account, request) => {
      const api = window.axiomDesktop?.agent;
      if (!api) throw new Error("Desktop is unavailable");
      return api.usageSummary(account, request);
    },
    resolvePermission: async (id, outcome) => window.axiomDesktop?.agent.resolvePermission(id, outcome),
    resolveElicitation: async (id, outcome) => window.axiomDesktop?.agent.resolveElicitation(id, outcome),
  }), [fullScreen]);
  return <ChatHostProvider host={host}><App /></ChatHostProvider>;
}

const root = document.getElementById("root");
if (!root) {
  throw new Error("Root element not found");
}

createRoot(root).render(
  <StrictMode>
    <DesktopHost />
  </StrictMode>,
);

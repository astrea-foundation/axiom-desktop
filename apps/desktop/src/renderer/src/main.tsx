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
    regenerateImage: async (thread, call) => {
      const api = window.axiomDesktop?.agent;
      if (!api) throw new Error("Desktop is unavailable");
      return api.regenerateImage(thread, call);
    },
    resolveImage: async (thread, image) => {
      const api = window.axiomDesktop?.agent;
      if (!api) throw new Error("Desktop is unavailable");
      return api.resolveImage(thread, image);
    },
    loadGeneratedImage: async (id) => {
      const api = window.axiomDesktop?.agent;
      if (!api) throw new Error("Desktop is unavailable");
      return api.loadGeneratedImage(id);
    },
    releaseGeneratedImage: (id) => window.axiomDesktop?.agent.releaseGeneratedImage(id),
    usageSummary: async (account, request) => {
      const api = window.axiomDesktop?.agent;
      if (!api) throw new Error("Desktop is unavailable");
      return api.usageSummary(account, request);
    },
    creditUsageSummary: async (account, request) => {
      const api = window.axiomDesktop?.agent;
      if (!api?.creditBilling) return null;
      try {
        const summary = await api.creditBilling({kind: "summary"}, account);
        if (summary.result.kind !== "summary" || !summary.result.value) return null;
      } catch (error) {
        if (error instanceof Error && /Restart Axiom with the updated native app/.test(error.message)) return null;
        throw error;
      }
      const response = await api.creditBilling({kind: "usage", period: request.period ?? "all_time", timezone: request.timezone ?? "UTC"}, account);
      if (response.result.kind !== "usage") throw new Error("Invalid credit usage response");
      return response.result.value;
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

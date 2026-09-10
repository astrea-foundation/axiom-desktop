import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "../src/App";
import { scenarios } from "./scenarios";
import "../src/styles.css";
import "./review.css";
// Preview is network-isolated. Provider controls and sample forms never reach a live account.
window.fetch = () => Promise.reject(new Error("Live network calls are disabled in the design preview."));
document.documentElement.dataset.theme = window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
function ReviewBar() {
  const current = scenarios.findIndex(({ url }) => url.split("#")[0] === window.location.pathname + window.location.search);
  return <aside className="review-bar" aria-label="Design preview controls">
    <span className="review-dot" /><strong>Design preview</strong><span className="review-data">Sample data</span>
    <label><span className="visually-hidden">Preview page</span><select value={current < 0 ? "" : current} onChange={(event) => window.location.assign(scenarios[Number(event.target.value)]!.url)}>
      {current < 0 && <option value="">Current page</option>}{scenarios.map((screen, index) => <option key={screen.name} value={index}>{screen.name}</option>)}
    </select></label>
  </aside>;
}
createRoot(document.getElementById("root")!).render(<StrictMode><App /><ReviewBar /></StrictMode>);

import type { ToolActivity } from "@axiom/axiom-acp-client";

export function redactToolText(text: string): string {
  return text
    .replace(/(?:axm_|axa_|axr_|sk_live_|sk_test_|whsec_)[A-Za-z0-9_-]+/g, "[REDACTED]")
    .replace(/Bearer\s+[A-Za-z0-9._~-]+/gi, "Bearer [REDACTED]")
    .replace(/Basic\s+[A-Za-z0-9+/=]+/gi, "Basic [REDACTED]");
}

function compact(text: string, limit: number): string {
  const safe = redactToolText(text).replace(/[\p{Cc}\p{Cf}]/gu, " ").replace(/\s+/g, " ").trim();
  const chars = Array.from(safe);
  return chars.length > limit ? `${chars.slice(0, limit).join("")}…` : safe;
}

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown> : null;
}

export function toolName(tool: ToolActivity): string {
  return tool.name ?? tool.title.split(" · ").at(-1) ?? tool.title;
}

export function toolFailed(tool: ToolActivity): boolean {
  return ["failed", "cancelled", "interrupted"].includes(tool.status);
}

export function isFetchTool(tool: ToolActivity): boolean {
  return toolName(tool) === "fetch_url" || tool.kind === "fetch"
    || (tool.kind === "web" && typeof object(tool.input)?.url === "string");
}

/** Friendly summaries of native errors, including already-saved tool rows. */
export function toolFailureReason(tool: ToolActivity): string | null {
  if (!toolFailed(tool)) return null;
  if (tool.status === "cancelled") return "Cancelled";
  if (tool.status === "interrupted") return "Interrupted";
  const fetch = isFetchTool(tool);
  const search = toolName(tool) === "web_search";
  // Read only bounded output text, never inputs or the whole raw object.
  const error = tool.content.slice(-8).map((entry) => {
    const block = object(entry);
    const text = block?.text ?? object(block?.content)?.text;
    return typeof text === "string" ? text.slice(0, 4096) : "";
  }).join("\n");
  if (/web is off/i.test(error)) return "Web is off";
  if (/sign in to Axiom again/i.test(error)) return "Sign in to Axiom again";
  if (/search limit reached/i.test(error)) return "Search limit reached";
  if (/local network destinations|non-public|private.*address|credential-bearing URLs/i.test(error)) return "Blocked for safety";
  const status = Number(error.match(/\b(?:HTTP(?: status)?|status(?: code)?)\s*[:=]?\s*(\d{3})\b/i)?.[1]);
  if (status === 403) return fetch ? "Blocked by website" : "Access denied";
  if (status === 401) return fetch ? "Website requires sign-in" : "Sign-in required";
  if (status === 404 || status === 410) return fetch ? "Page not found" : "Not found";
  if (status === 429) return search ? "Search limit reached" : "Too many requests";
  if (status === 451) return "Unavailable for legal reasons";
  if (status === 408 || status === 504 || /timed?\s*out|timeout/i.test(error)) return "Request timed out";
  if ((status >= 500 && status <= 599) || /web search.*(?:unavailable|temporarily unavailable)/i.test(error)) {
    return fetch ? "Website unavailable" : search ? "Search service unavailable" : "Service unavailable";
  }
  if (/too many redirects|redirect limit exhausted/i.test(error)) return "Too many redirects";
  if (/invalid redirect|redirect has no valid Location/i.test(error)) return "Invalid redirect";
  if (/body exceeds size limit/i.test(error)) return "Page too large";
  if (/content type is not text-readable/i.test(error)) return "Unsupported page format";
  if (/invalid fetch URL|fetch URL has no host|URL has no usable port|fetch permits only http/i.test(error)) return "Invalid URL";
  if (/DNS lookup failed|DNS returned no addresses/i.test(error)) return "Website not found";
  if (/certificate|TLS handshake|SSL/i.test(error)) return "Secure connection failed";
  if (/permission denied/i.test(error)) return "Not permitted";
  return fetch ? "Couldn’t fetch page" : search ? "Search failed" : "Action failed";
}

/** Match the TUI's phase-aware action + target labels, including older records. */
export function toolLabel(tool: ToolActivity): string {
  const name = toolName(tool);
  const input = object(tool.input);
  const failed = toolFailed(tool);
  const reason = toolFailureReason(tool);
  const done = tool.status === "completed";
  let label: string;
  if (name === "web_search") {
    const verb = failed ? "Failed to search the web" : done ? "Searched the web" : "Searching the web";
    const query = typeof input?.query === "string" ? compact(input.query, 140) : "";
    label = reason
      ? `${reason}: Web search${query ? ` for “${query}”` : ""}`
      : query ? `${verb} for “${query}”` : verb;
  } else if (isFetchTool(tool)) {
    const verb = failed ? "Failed to fetch" : done ? "Fetched" : "Fetching";
    let target = "a page";
    if (typeof input?.url === "string") {
      try {
        const url = new URL(input.url);
        // Credentials, query parameters, and fragments aren't useful labels.
        target = compact(`${url.host}${url.pathname === "/" ? "" : url.pathname}`, 140);
      } catch { /* Keep a safe generic label for malformed URLs. */ }
    }
    label = reason ? `${reason}: ${target}` : `${verb} ${target}`;
  } else {
    // New snapshots already use the native TUI formatter for every tool.
    label = tool.title.replace(/ · [a-z][a-z0-9_]*$/, "").replace(/`([^`]+)`/g, "$1");
    if (label === name && /^[a-z][a-z0-9_]*$/.test(name)) {
      label = `${failed ? "Failed to run" : done ? "Ran" : "Running"} ${name.replace(/_/g, " ")}`;
    }
    if (reason) label = `${reason}: ${label}`;
  }
  return compact(label, 220);
}

export type SearchSource = { title: string; url: string; host: string; snippet: string };

export function toolSearchSources(tool: ToolActivity): SearchSource[] | null {
  if (toolName(tool) !== "web_search" || tool.status !== "completed") return null;
  for (const entry of tool.content) {
    const block = object(entry);
    const text = block?.text ?? object(block?.content)?.text;
    if (typeof text !== "string" || text.length > 1_048_576) continue;
    try {
      const value = object(JSON.parse(text));
      if (!Array.isArray(value?.results)) continue;
      const sources: SearchSource[] = [];
      for (const entry of value.results.slice(0, 20)) {
        const result = object(entry);
        if (typeof result?.url !== "string" || result.url.length > 4096) continue;
        try {
          const url = new URL(result.url);
          if (!["https:", "http:"].includes(url.protocol) || url.username || url.password) continue;
          sources.push({
            url: url.href, host: url.host,
            title: compact(typeof result.title === "string" ? result.title : url.host, 200),
            snippet: compact(typeof result.snippet === "string" ? result.snippet : "", 320),
          });
        } catch { /* Untrusted tool output cannot introduce executable links. */ }
      }
      return sources;
    } catch { /* Other tool output is still available in bounded raw details. */ }
  }
  return null;
}

/** @jsxRuntime automatic */
import { Globe2 } from "lucide-react";
import type { DesktopMcpServer, ToolActivity } from "@axiom/chat-core/viewTypes";

export function mcpHost(destination?: string | null): string | null {
  if (!destination) return null;
  try {
    const url = new URL(destination);
    return ["https:", "http:"].includes(url.protocol) && !url.username && !url.password ? url.host : null;
  } catch { return null; }
}

export function RemoteMcpNotice({ destination, compact = false }: { destination?: string | null; compact?: boolean }) {
  const host = mcpHost(destination);
  return <div className="flex items-start gap-2.5 rounded-xl border border-warning-border bg-warning-soft p-3 text-[12px] leading-5 text-warning-strong" data-remote-mcp-notice>
    <Globe2 size={15} className="mt-0.5 shrink-0" aria-hidden="true" />
    <div>{compact ? <p>Selected remote tools can send conversation or file content to external services outside Axiom’s attested environment.</p> : <>
      <p className="font-medium">Remote tools share data outside Axiom’s attested environment</p>
      <p className="mt-1">When a remote tool runs, its inputs are sent to {host ? <strong className="break-all">{host}</strong> : "the service you connect"}. These inputs may include content from your conversation or files. Axiom does not attest how this service processes or stores that data.</p>
    </>}</div>
  </div>;
}

export function McpSourceLabel({ source }: { source: NonNullable<ToolActivity["mcpSource"]> }) {
  const host = mcpHost(source.destination);
  return <span className="inline-flex min-w-0 items-center gap-1 text-[11px] text-[var(--color-text-tertiary)]">
    {host ? <Globe2 size={12} className="shrink-0" aria-hidden="true" /> : null}
    <span className="truncate">{host ? `Remote · ${host}` : "Local process"}</span>
  </span>;
}

export function McpPrivacyDetails({ servers = [], selectedTools = [], session }: {
  servers?: DesktopMcpServer[]; selectedTools?: string[]; session: { timeline: Array<{ tool?: ToolActivity }> };
}) {
  const selected = new Set(selectedTools);
  const enabled = [...new Set(servers.filter(s => s.enabled && s.remote && s.tools.some(t => selected.has(t.name)))
    .map(s => mcpHost(s.remote?.url)).filter((h): h is string => !!h))];
  const used = [...new Set(session.timeline.filter(item => item.tool?.started === true || item.tool?.status === "completed")
    .map(item => mcpHost(item.tool?.mcpSource?.destination)).filter((h): h is string => !!h))];
  return <section aria-label="External services" className="mt-4 rounded-xl bg-[var(--wash-row)] p-3 text-[12px] leading-5">
    <h3 className="font-medium">External services</h3>
    <p className="mt-1 text-[var(--color-text-secondary)]">Remote MCP tool calls are outside Axiom’s attested environment. Each service receives the inputs sent to its tools. View tool activity to see destinations and shared inputs.</p>
    <p className="mt-2 break-words"><span className="text-[var(--color-text-tertiary)]">Enabled for this conversation: </span>{enabled.join(", ") || "None"}</p>
    <p className="mt-1 break-words"><span className="text-[var(--color-text-tertiary)]">Requests attempted in loaded history: </span>{used.join(", ") || "None recorded"}</p>
    <p className="mt-2 text-[var(--color-text-tertiary)]">Local MCP processes also run outside the attested model environment and can access your computer and the network.</p>
  </section>;
}
